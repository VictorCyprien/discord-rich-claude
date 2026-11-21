//! Bibliothèque de discord-rich-claude : le daemon, l'installation et la ligne de commande.

pub mod bundle;
pub mod carte;
pub mod chemins;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod installation;
pub mod journal;
pub mod menu;
pub mod presence;
pub mod serveur;
pub mod sessions;
pub mod tokens;

#[cfg(test)]
mod aides_tests;
