#[tokio::main]
async fn main() {
    if let Err(error) = bgdeobf::bot::run().await {
        eprintln!("{}", error.message);
        std::process::exit(1);
    }
}
