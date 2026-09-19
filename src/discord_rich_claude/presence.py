"""Connexion à Discord : publication à débit limité, reconnexion. Voir la spec, sections 4.5, 5 et 9."""
import asyncio
import logging
import math
import time
from collections.abc import Callable
from typing import Protocol

from pypresence import AioPresence

from .carte import Carte

log = logging.getLogger(__name__)

# Discord documente « one update per 15 seconds » ; au-delà, la présence peut
# être vidée au lieu d'être mise en file (discord-api-docs#668). Valeur confirmée en Task 1.
INTERVALLE_MIN_S = 15.0
REESSAI_S = 15.0

# pypresence 4.6.2 borne l'ouverture du socket (`connection_timeout`) mais pas la
# lecture de la réponse à la poignée de main (`BaseClient.handshake`, `sock_reader.read`
# sans timeout) : un pair qui accepte la connexion sans jamais répondre bloque
# indéfiniment `connecter`, donc toute la boucle carte à 1 s. On borne nous-mêmes.
DELAI_POIGNEE_MAIN_S = 5.0


class ClientDiscord(Protocol):
    async def connecter(self) -> None: ...
    async def publier(self, carte: Carte) -> None: ...
    async def effacer(self) -> None: ...
    async def fermer(self) -> None: ...


class ClientPypresence:
    def __init__(self, client_id: str) -> None:
        self._rpc = AioPresence(client_id, connection_timeout=5, response_timeout=5)

    async def connecter(self) -> None:
        try:
            await asyncio.wait_for(self._rpc.connect(), DELAI_POIGNEE_MAIN_S)
        except Exception:
            self._fermer_apres_echec()
            raise

    def _fermer_apres_echec(self) -> None:
        # La poignée de main peut échouer (timeout, ID invalide, ...) après que le
        # socket a été ouvert : `handshake()` ne le referme jamais, ce qui fuit un
        # descripteur par tentative. On le fait à sa place, sans jamais lever : un
        # échec de fermeture ne doit pas masquer l'échec de connexion d'origine.
        ecrivain = self._rpc.sock_writer
        if ecrivain is None:
            return
        try:
            ecrivain.close()
        except Exception as e:
            log.warning("fermeture du socket après échec de connexion : %s", type(e).__name__)

    async def publier(self, carte: Carte) -> None:
        await self._rpc.update(
            details=carte.details,
            state=carte.state,
            start=carte.debut,
            large_image=carte.large_image,
            large_text=carte.large_text,
        )

    async def effacer(self) -> None:
        await self._rpc.clear()

    async def fermer(self) -> None:
        # Reprend `AioPresence.close()` (pypresence 4.6.2) sans son `self.loop.close()` :
        # `self.loop` y est la boucle asyncio en cours (capturée par `get_event_loop()`
        # à la connexion), la fermer lève `RuntimeError: Cannot close a running event
        # loop`. Sur macOS/Linux, `close()` ne ferme jamais le socket non plus (seul
        # win32 appelle `_call_connection_lost`) : on ferme nous-mêmes le lecteur.
        try:
            self._rpc.send_data(2, {"v": 1, "client_id": self._rpc.client_id})
        except Exception as e:
            log.warning("fermeture IPC Discord incomplète (%s) : %s", "trame de fermeture", type(e).__name__)
        ecrivain = self._rpc.sock_writer
        if ecrivain is None:
            return
        try:
            await ecrivain.drain()
        except Exception as e:
            log.warning("fermeture IPC Discord incomplète (%s) : %s", "vidage du tampon", type(e).__name__)
        try:
            ecrivain.close()
        except Exception as e:
            log.warning("fermeture IPC Discord incomplète (%s) : %s", "fermeture du socket", type(e).__name__)


class Presence:
    def __init__(
        self,
        fabrique: Callable[[str], ClientDiscord],
        intervalle_min_s: float = INTERVALLE_MIN_S,
        reessai_s: float = REESSAI_S,
        horloge: Callable[[], float] = time.monotonic,
    ) -> None:
        self._fabrique = fabrique
        self._intervalle_min_s = intervalle_min_s
        self._reessai_s = reessai_s
        self._horloge = horloge
        self._client: ClientDiscord | None = None
        self._client_id: str | None = None
        self._derniere: Carte | None = None
        self._dernier_envoi = -math.inf
        self._dernier_essai = -math.inf
        # Type de l'exception en cours (None si le dernier essai a réussi) : le
        # même pattern que `daemon._boucle`, pour ne pas noyer le journal d'un
        # essai toutes les 15 s tant que Discord est injoignable.
        self._type_erreur_en_cours: type[BaseException] | None = None

    @property
    def connecte(self) -> bool:
        return self._client is not None

    async def soumettre(self, carte: Carte | None, client_id: str | None) -> None:
        if carte is None or client_id is None or client_id != self._client_id:
            await self.fermer()
        if carte is None or client_id is None:
            return
        maintenant = self._horloge()
        if self._client is None:
            if maintenant - self._dernier_essai < self._reessai_s:
                return
            self._dernier_essai = maintenant
            try:
                client = self._fabrique(client_id)
                await client.connecter()
            except Exception as e:
                self._signaler_echec("Discord injoignable, nouvel essai dans %.0f s", e)
                return
            self._client, self._client_id = client, client_id
            self._type_erreur_en_cours = None
            log.info("connecté à Discord")
        if carte == self._derniere or maintenant - self._dernier_envoi < self._intervalle_min_s:
            return
        try:
            await self._client.publier(carte)
        except Exception as e:
            self._signaler_echec("publication refusée, reconnexion dans %.0f s", e)
            client, self._client, self._client_id, self._derniere = self._client, None, None, None
            self._dernier_essai = maintenant
            try:
                await client.fermer()
            except Exception:
                pass
            return
        self._derniere, self._dernier_envoi = carte, maintenant

    def _signaler_echec(self, gabarit: str, e: Exception) -> None:
        # Tant que Discord reste injoignable (fermé, ID invalide...), un essai
        # toutes les 15 s ne doit produire qu'une seule ligne WARNING par type
        # d'exception rencontré ; les répétitions du même type partent en DEBUG.
        # Jamais `str(e)` : seul le type, comme `daemon._boucle`.
        if type(e) is not self._type_erreur_en_cours:
            log.warning(gabarit + " : %s", self._reessai_s, type(e).__name__)
            self._type_erreur_en_cours = type(e)
        else:
            log.debug(gabarit + " : %s", self._reessai_s, type(e).__name__)

    async def fermer(self) -> None:
        client, self._client, self._client_id = self._client, None, None
        self._derniere, self._dernier_envoi = None, -math.inf
        if client is None:
            return
        for action in (client.effacer, client.fermer):
            try:
                await action()
            except Exception as e:
                log.warning("fermeture de la connexion Discord : %s", e)
