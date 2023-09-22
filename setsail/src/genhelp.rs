use clap::CommandFactory;
use setsail::commands::network::Network;

/// This is a test for a documentation generator

fn main() {
    dump_command::<Network>("network");
}

fn dump_command<T: CommandFactory>(name: &'static str) {
    let cmd = T::command().name(name);
    // cmd.print_help();
    cmd.get_arguments().for_each(|arg| {
        println!(
            "{}\n{}\n{}\n{}",
            arg.get_id(),
            arg.get_all_aliases().unwrap_or_default().join(", "),
            arg.get_help_heading().unwrap_or_default(),
            arg.get_long_help().unwrap_or_default()
        );
    });
}
