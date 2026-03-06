use musli::{Decode, Encode};

#[derive(Decode, Encode)]
pub struct Config {
    pub name: String,
}
