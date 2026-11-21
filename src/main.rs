fn main() {
    std::process::exit(discord_rich_claude::cli::principal(
        std::env::args().skip(1).collect(),
    ));
}
