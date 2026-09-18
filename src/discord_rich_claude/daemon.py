"""Assemblage du daemon : serveur, état, tokens, carte, Discord. Voir la spec, sections 3 et 4.4."""
import asyncio
import logging
import os
import time
from collections.abc import Callable
from contextlib import suppress
from logging.handlers import RotatingFileHandler
from pathlib import Path

from .carte import Carte, Stats, construire_carte, est_vue_b
from .config import ConfigSurveillee
from .presence import Presence
from .serveur import demarrer_serveur
from .sessions import Evenement, Registre, pid_vivant
from .tokens import SuiviTokens, minuit_local

log = logging.getLogger(__name__)

PERIODE_CONTROLE_S = 30
PERIODE_CARTE_S = 1


def configurer_logs(dossier: Path) -> None:
    dossier.mkdir(parents=True, exist_ok=True)
    gestionnaire = RotatingFileHandler(
        dossier / "daemon.log", maxBytes=1_000_000, backupCount=3, encoding="utf-8"
    )
    gestionnaire.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s %(message)s"))
    racine = logging.getLogger()
    racine.addHandler(gestionnaire)
    racine.setLevel(os.environ.get("DRC_LOG", "INFO").upper())


class Daemon:
    def __init__(
        self,
        config: ConfigSurveillee,
        presence: Presence,
        suivi: SuiviTokens,
        projets_claude: Path,
        pid_vivant: Callable[[int], bool] = pid_vivant,
        horloge: Callable[[], float] = time.time,
    ) -> None:
        self.config = config
        self.presence = presence
        self.suivi = suivi
        self.projets_claude = projets_claude
        self.pid_vivant = pid_vivant
        self.horloge = horloge
        self.registre = Registre()
        self._par_session: dict[str, tuple[str | None, int]] = {}
        self._tokens_jour = 0
        self._verrou = asyncio.Lock()

    def recevoir(self, ev: Evenement) -> None:
        niveau = logging.INFO if ev.nom in ("SessionStart", "SessionEnd") else logging.DEBUG
        log.log(niveau, "%s session=%s", ev.nom, ev.session_id[:8])
        self.registre.appliquer(ev)

    def etat(self) -> dict:
        return {"discord": self.presence.connecte, "sessions": len(self.registre.sessions)}

    def carte(self) -> Carte | None:
        maintenant = self.horloge()
        cfg = self.config.valeur
        session = self.registre.session_affichee()
        modele, tokens = self._par_session.get(session.session_id, (None, 0)) if session else (None, 0)
        stats = Stats(modele=modele, tokens_session=tokens, tokens_jour=self._tokens_jour)
        return construire_carte(
            self.registre, cfg, stats, maintenant, est_vue_b(maintenant, cfg.rotation_s)
        )

    async def controler(self) -> None:
        # Le verrou garantit un seul appelant à la fois pour SuiviTokens, qui n'est pas thread-safe.
        async with self._verrou:
            maintenant = self.horloge()
            self.config.recharger_si_change()
            inactif_s = self.config.valeur.inactif_apres_min * 60
            self.registre.purger(maintenant, inactif_s, self.pid_vivant)
            transcripts = {
                sid: Path(s.transcript_path)
                for sid, s in self.registre.sessions.items()
                if s.transcript_path
            }
            minuit = minuit_local(maintenant)
            # Lecture disque dans un thread : les hooks continuent d'être servis.
            self._par_session, self._tokens_jour = await asyncio.to_thread(
                self._compter, transcripts, minuit
            )

    def _compter(
        self, transcripts: dict[str, Path], minuit: float
    ) -> tuple[dict[str, tuple[str | None, int]], int]:
        self.suivi.rafraichir(transcripts.values(), minuit)
        par_session = {
            sid: (self.suivi.modele(t), self.suivi.tokens_session(t)) for sid, t in transcripts.items()
        }
        return par_session, self.suivi.tokens_jour(minuit)

    async def executer(self, chemin_socket: Path, arret: asyncio.Event) -> None:
        serveur = await demarrer_serveur(chemin_socket, self.recevoir, self.etat, self.projets_claude)
        log.info("daemon à l'écoute sur %s", chemin_socket)
        taches = [
            asyncio.create_task(self._boucle(self.controler, PERIODE_CONTROLE_S, "contrôle")),
            asyncio.create_task(self._boucle(self._publier, PERIODE_CARTE_S, "carte")),
        ]
        try:
            await arret.wait()
        finally:
            for tache in taches:
                tache.cancel()
            await asyncio.gather(*taches, return_exceptions=True)
            serveur.close()
            await serveur.wait_closed()
            await self.presence.fermer()
            with suppress(FileNotFoundError):
                chemin_socket.unlink()
            log.info("daemon arrêté")

    async def _publier(self) -> None:
        await self.presence.soumettre(self.carte(), self.config.valeur.client_id)

    @staticmethod
    async def _boucle(action: Callable, periode_s: float, nom: str) -> None:
        while True:
            try:
                await action()
            except Exception:
                log.exception("%s en erreur", nom)
            await asyncio.sleep(periode_s)
