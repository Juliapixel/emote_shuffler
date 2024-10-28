use std::error::Error;

use clap::Parser;
use emote_shuffler::{cli::Args, SevenTvGqlClient};
use log::error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let args = Args::parse();

    let client = SevenTvGqlClient::new(dotenvy::var("SEVENTV_TOKEN").unwrap());
    let set = client.get_user_emote_set(&args.username).await?;

    match client.shuffle_set(set.id).await {
        Ok(_) => (),
        Err(e) => error!("{e}"),
    };

    Ok(())
}
