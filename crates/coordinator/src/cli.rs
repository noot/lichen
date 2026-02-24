use clap::Parser;

#[derive(Parser)]
pub struct Args {
    #[arg(long, default_value = "3000")]
    pub port: u16,

    /// Alpha weight for information score in RBTS
    #[arg(long, default_value = "1.0")]
    pub alpha: f64,

    /// Beta weight for prediction score in RBTS
    #[arg(long, default_value = "1.0")]
    pub beta: f64,
}
