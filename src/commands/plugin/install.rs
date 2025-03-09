use clap::{value_parser, Args};

use super::version::PluginVersion;

#[derive(Debug, Args)]
pub struct PluginInstallArgs {
    /// Space-separated names and versions of plugins to install, e.g. `plugin1 plugin2@version`
    #[arg(value_parser = value_parser!(PluginVersion))]
    plugins: Vec<PluginVersion>,
}
