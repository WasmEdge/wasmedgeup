use clap::Args;

use super::version::PluginVersion;

#[derive(Debug, Args)]
pub struct PluginRemoveArgs {
    /// Space-separated names and versions of plugins to remove, e.g. `plugin1 plugin2@version`
    #[arg(value_parser = clap::value_parser!(PluginVersion))]
    plugins: Vec<PluginVersion>,
}
