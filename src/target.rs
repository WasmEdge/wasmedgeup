use clap::ValueEnum;

#[derive(Debug, Clone, ValueEnum)]
pub enum TargetOS {
    Linux,
    Ubuntu,
    #[value(alias = "macos")]
    Darwin,
    Windows,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum TargetArch {
    #[value(name = "x84_64", alias("x64"))]
    X84_64,
    #[value(alias("amd64"))]
    Aarch64,
}
