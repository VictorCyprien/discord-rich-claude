//! Sonde manuelle de la crate `discord-presence` : à lancer par Victor seulement
//! (elle touche le vrai profil Discord). L'identifiant client n'est jamais affiché.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_presence::Client;

fn lire_client_id() -> Option<u64> {
    let chemin =
        PathBuf::from(std::env::var_os("HOME")?).join(".config/discord-rich-claude/config.toml");
    let texte = std::fs::read_to_string(chemin).ok()?;
    let config: toml::Table = texte.parse().ok()?;
    match config.get("client_id")? {
        toml::Value::String(s) => s.trim().parse().ok(),
        toml::Value::Integer(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

fn depuis_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn activite(client: &mut Client) -> String {
    let debut = depuis_epoch();
    let resultat = client.set_activity(|a| {
        a.details("Sonde Rust")
            .state("essai")
            .timestamps(|t| t.start(debut))
            .assets(|a| a.large_image("logo").large_text("Claude Code"))
    });
    match resultat {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("erreur : {e:?}"),
    }
}

fn main() {
    let Some(client_id) = lire_client_id() else {
        eprintln!("client_id introuvable ou invalide dans la config");
        std::process::exit(2);
    };

    let mut client = Client::new(client_id);
    let (tx_pret, rx_pret) = mpsc::channel::<()>();
    let tx_pret = std::sync::Mutex::new(tx_pret);
    client
        .on_ready(move |_| {
            println!("événement : Ready");
            if let Ok(tx) = tx_pret.lock() {
                let _ = tx.send(());
            }
        })
        .persist();
    client
        .on_connected(|_| println!("événement : Connected"))
        .persist();
    client
        .on_disconnected(|_| println!("événement : Disconnected"))
        .persist();
    client.on_error(|_| println!("événement : Error")).persist();
    client.start();

    if rx_pret.recv_timeout(Duration::from_secs(5)).is_err() {
        println!("Ready non reçu en 5 s");
    }

    println!("set_activity : {}", activite(&mut client));
    sleep(Duration::from_secs(20));

    match client.clear_activity() {
        Ok(_) => println!("clear_activity : ok"),
        Err(e) => println!("clear_activity : erreur : {e:?}"),
    }
    sleep(Duration::from_secs(16));

    println!("second set_activity : {}", activite(&mut client));

    println!("fenêtre de 60 s : quitter puis relancer Discord");
    sleep(Duration::from_secs(60));

    match client.shutdown() {
        Ok(()) => println!("shutdown : ok"),
        Err(e) => println!("shutdown : erreur : {e:?}"),
    }
}
