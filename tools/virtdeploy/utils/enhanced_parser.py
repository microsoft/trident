import argparse
import os
import textwrap
from types import ModuleType
from typing import Callable, Dict, List
import importlib.util
import importlib
import pkgutil


class SubCommand:
    def __init__(
        self,
        name: str,
        parser_callback: Callable[[argparse.ArgumentParser], None],
        action_callback: Callable[[argparse.Namespace], None],
        summary: str = "",
        description: str = "",
        **kwargs,
    ) -> None:
        self.name = name
        self.parser_callback = parser_callback
        self.action_callback = action_callback
        self.description = description
        self.summary = summary
        self.kwargs = kwargs


class EnhancedArgumentParser(argparse.ArgumentParser):
    def __init__(
        self,
        prog=None,
        usage=None,
        summary=None,
        description=None,
        epilog=None,
        parents=[],
        formatter_class=argparse.HelpFormatter,
        prefix_chars="-",
        fromfile_prefix_chars=None,
        argument_default=None,
        conflict_handler="error",
        allow_abbrev=True,
    ) -> None:
        # Create super, but do NOT add the default help
        super().__init__(
            prog,
            usage,
            description,
            epilog,
            parents,
            formatter_class,
            prefix_chars,
            fromfile_prefix_chars,
            argument_default,
            conflict_handler,
            False,
            allow_abbrev,
        )

        self.summary = summary
        # Add out custom help
        self.add_argument(
            "-h",
            "--help",
            action=SubCommandHelpAction,
            default=argparse.SUPPRESS,
            help=("show this help message and exit"),
        )

        # Save required choice args:
        self._choice_args = {}

    def add_enhanced_choice_argument(self, *args, choices: Dict[str, str], **kwargs):
        action = self.add_argument(*args, choices=choices.keys(), **kwargs)
        self._choice_args[action] = choices

    def add_subcommands(
        self,
        cmds: Dict[str, SubCommand],
        dest: str,
        helpstr: str,
        metavar: str,
    ) -> None:
        spf = self.add_subparsers(
            dest=dest, help=helpstr, metavar=metavar, required=True
        )
        for name, cmd in cmds.items():
            sp = spf.add_parser(
                name,
                summary=cmd.summary,
                description=cmd.description,
                **cmd.kwargs,
            )
            cmd.parser_callback(sp)


class SubCommandHelpAction(argparse._HelpAction):
    def __call__(self, parser, namespace, values, option_string) -> None:
        # Long description
        if hasattr(parser, "long_description") and parser.long_description:
            print(
                textwrap.fill(
                    parser.long_description,
                    os.get_terminal_size().columns,
                    replace_whitespace=False,
                )
            )
            print("")

        # Regular help
        parser.print_help()

        # Print subparser info
        subparsers = [
            item
            for item in parser._actions
            if isinstance(item, argparse._SubParsersAction)
        ]
        for item in subparsers:
            print(f"\nOptions available for {item.metavar}:")
            descriptions = format_descriptions(
                {name: data.summary for name, data in item.choices.items()}
            )
            print("\n".join(descriptions))

        # Print choice args
        for item, choices in parser._choice_args.items():
            name = item.metavar if item.metavar else item.dest
            print(f"\nOptions available for {name}:")
            descriptions = format_descriptions(choices)
            print("\n".join(descriptions))
        parser.exit()


def format_descriptions(
    choices: Dict[str, str], base_indent=4, separator="-"
) -> List[str]:
    descriptions: List[str] = []
    longest = max(map(lambda x: len(x), choices.keys()))
    subsequent_indent = base_indent + longest + len(separator) + 2
    for name in sorted(choices.keys()):
        tmp = name.ljust(longest)
        if choices[name]:
            tmp += f" {separator} {choices[name]}"
        descriptions.append(
            textwrap.fill(
                tmp,
                os.get_terminal_size().columns,
                initial_indent=" " * base_indent,
                subsequent_indent=" " * subsequent_indent,
            )
        )
    return descriptions


def FindSubCommands(module: ModuleType) -> Dict[str, SubCommand]:
    commands = {}
    for package in pkgutil.iter_modules(module.__path__):
        # Construct the full module path
        full_module_name = f"{module.__name__}.{package.name}"
        try:
            spec = importlib.util.find_spec(full_module_name)
            if spec is not None:
                m = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(m)
            else:
                m = importlib.import_module(full_module_name)
            metadata: SubCommand = m.CMD_METADATA
            assert metadata.name not in commands, "COMMAND NAMES CANNOT BE DUPLICATED!"
            commands[metadata.name] = metadata
        except (ImportError, AttributeError) as e:
            # Skip modules that don't have CMD_METADATA or can't be imported
            continue
    return commands
