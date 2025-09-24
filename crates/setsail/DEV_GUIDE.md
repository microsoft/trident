# Setsail Development Guide

## Structure

```
setsail
├── Cargo.toml          # Cargo configuration
├── DEV_GUIDE.md        # This file
└── src
    ├── commands        # Kickstart command parsers
    │   ├── mod.rs      # Declaration of all command parsers
    │   └── [ . . . ]   # Per-command parser submodules
    ├── data.rs         # Parsed data representation
    ├── docbuilder      # Documentation builder
    │   └── [ . . . ]
    ├── errors.rs       # Error type definitions
    ├── handlers.rs     # Section handler traits & basic implementations
    ├── lib.rs          # Main library file
    ├── load.rs         # File and string loading
    ├── makedocs.rs     # Documentation generation
    ├── parser.rs       # Main parser
    ├── preprocess.rs   # Preprocessor
    ├── sections        # Section handlers
    │   └── [ . . . ]
    ├── test_files      # Test files
    │   └── [ . . . ]
    ├── translator      # Translation to Host Configuration
    │   └── [ . . . ]
    └── types.rs
```

## Implementing a New Command

For a comprehensive example, see the implementation of the `network` command.

1. Create a new file in `src/commands` with the name of the command (you can
   use `template.rs` as a base). Example:

    ```bash
    touch src/commands/mycommand.rs
    ```

2. Implement a [clap](https://docs.rs/clap/latest/clap/index.html) derive parser
   struct for your command. Example:

    ```rust
    // mycommand.rs
    use clap::Parser;

    use crate::{data::ParsedData, types::KSLine, SetsailError};  

    #[derive(Parser, Debug)]
    #[command(name = "mycommand", aliases = &["cmd"])]
    pub struct MyCommand {
        // Internal
        #[clap(skip)]
        pub line: KSLine,
   
        /// The docstring gets transformed into the help message
        #[arg(long)]
        pub name: String,

        // . . .
    }
    ```

    - The struct must have a `line` field of type `KSLine`. This is used to
      generate error messages.
    - Add the command name and aliases (if any) used in kickstart to the
      `#[command]` attribute.
    - Add all the fields you want to parse as command line arguments. See the
      [clap documentation](https://docs.rs/clap/latest/clap/index.html) for
      more information.
    - The struct must implement `CommandHandler` (see below).

3. Add your parsed data to the `ParsedData` struct in `src/data.rs`. Example:

    ```rust
    #[derive(Debug, Default)]
    pub struct ParsedData {
        // . . .
        pub mycommand: Option<MyCommand>,
    }
    ```

    - You will store all _successfully_ parsed data in this struct.
    - _Generally_ one field should exist per command.
    - If you need to parse multiple instances of a command, use a `Vec` or
      similar container.
    - The field must implement `Default`.

4. Implement the `setsail::commands::CommandHandler` trait for your command. Example:

    ```rust
    use super::CommandHandler; 

    impl HandleCommand for MyCommand {
        /// Handle the command
        /// Arguments:
        ///     self: The command struct
        ///     line: The line the command was found on, used for error messages
        ///     data: The parsed data, add parsed data here
        fn handle(mut self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
            // Do stuff with the parsed data (self)
            // Validate parsing, enforce requirements, checks, etc.
            // Return an error/warning if something goes wrong
            // Transform data as needed
            
            // If everything went well, add whatever you need to the data struct.
            // In this example we're storing the command struct itself.
            // Generally we want to save the line the command was found
            // on so that the translator can point to it in case any error comes
            // up.

            self.line = line;
            data.mycommand = Some(self);
            Ok(())
        }
    }
    ```

5. Add your command module to `src/commands/mod.rs`.

    ```rust
    // src/commands/mod.rs
    pub mod mycommand;
    ```

6. Add your command to the command handler in `src/commands/mod.rs`. Please keep
   the list in alphabetical order!

    ```rust
    impl<'a> CommandHandler<'a> {
        pub fn handle(mut self) -> Result<(), SetsailError> {
            match self.tokens[0].as_str() {
                // Commands we understand
                // . . .
                "mycommand" => self.dispatch::<mycommand::MyCommand>(),
                // . . .
            }
        }
    }
    ```

7. Implement a translator for your command. Create a new file in
   `src/translator` with the name of your command. Example:

    ```bash
    touch src/translator/mycommand.rs
    ```

8. Implement a function of prototype:

    ```rust
    pub fn translate(
        input: &ParsedData,
        hc: &mut HostConfiguration,
        errors: &mut Vec<SetsailError>)
    ```

    Do all required logic to translate your command into host configuration.
    Add any errors to the `errors` vector.

9. Add your command to the documentation builder in `src/docbuilder.rs`. Please
   keep the list in alphabetical order!

    ```rust
    // src/docbuilder.rs

    // ADD NEW COMMANDS HERE
    use setsail::commands::{
        mycommand::MyCommand,
        network::Network, 
        partition::Partition
    };

    fn main() {
        let args = Args::parse();

        // USE `with_command` TO ADD NEW COMMANDS TO THE DOC
        let doc = DocBuilder::new()
            .with_command::<MyCommand>()
            .with_command::<Network>()
            .with_command::<Partition>()
            .build();
    }
    ```

## Documentation Generation Details

The documentation is generated by the `DocBuilder` struct in `src/docbuilder.rs`.

It extracts clap's help messages from the command structs and uses them to
create a markdown document.

ALL visible fields MUST be documented.

Document fields like this:

```rust
pub struct MyCommand {
    /// <SHORT ONE-LINE DESCRIPTION>
    /// 
    /// <LONG MULTI-LINE DESCRIPTION>
    ///
    /// <EXTRA-INFO>
    pub bootproto: BootProto,
}
```

**NOTE:** The empty line between the short and long description is important!

Right now the only things you need to add in extra info are:

- Other field requirements (i.e. this flag is only valid if anothe flag is present)

    ```rust
    /// Requires: <FIELD>
    ```
  
- Possible Values when the field is an enum that does NOT implement `ValueEnum`

    ```rust
    /// Possible Values: 
    /// - <VALUE1>
    /// - <VALUE2>
    /// - <VALUE3>
    ```

## Complex Markdown in Docstring

If you have a list or anything more complex in markdown, clap will mess with the
format (See
[Clap Derive: Doc Comments](https://docs.rs/clap/latest/clap/_derive/index.html#doc-comments))
unless you add the arrtibute `verbatim_doc_comment`.

For args: `#[arg(verbatim_doc_comment)]`

For commands: `#[command(verbatim_doc_comment)]`
