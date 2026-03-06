use musli::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub name: String,
}

#[derive(Encode, Decode)]
pub struct MusliConfig {
    #[musli(with = musli::serde)]
    pub name: Config,
}

fn main() {}
