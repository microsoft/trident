# Helpful warning
if __name__ == "__main__":
    print("Please do not run this file directly!")
    exit()

import argparse
import logging
import os
import re
import sys

from virtdeploy import commands as cmds

from virtdeploy.utils import EnhancedArgumentParser, FindSubCommands, main_location

logging.basicConfig(level=logging.INFO)
log = logging.getLogger(__name__)


def run():
    parser = EnhancedArgumentParser(
        formatter_class=argparse.ArgumentDefaultsHelpFormatter
    )

    # Inject hidden argument with the path to the repo root
    parser.add_argument(
        "--location", help=argparse.SUPPRESS, default=main_location(), required=False
    )

    # Inject hidden argument to allow running this script as root.
    parser.add_argument(
        "--allow-root", help=argparse.SUPPRESS, action="store_true", required=False
    )

    # Global argument: prefix for virtual resources
    prefix_format = r"[A-Za-z0-9-]*"
    prefix_default = "virtdeploy"
    parser.add_argument(
        "--nameprefix",
        help=f"Create all resources with this name prefix. {prefix_format}",
        default=prefix_default,
    )

    parser.add_argument(
        "--debug",
        help="Enable debug logging",
        action="store_true",
        default=False,
    )

    # Find all commands in the commands module
    commands = FindSubCommands(cmds)

    # Add them to the parser
    parser.add_subcommands(
        commands, dest="command", helpstr="Command to run", metavar="CMD"
    )

    # Run parser
    args = parser.parse_args()

    if args.debug:
        logging.basicConfig(level=logging.DEBUG)
        for logger in [
            logging.getLogger(name) for name in logging.root.manager.loggerDict
        ]:
            logger.setLevel(logging.DEBUG)
        log.debug("Debug logging enabled!")

    if not args.allow_root:
        # Ensure we are NOT root
        if os.geteuid() == 0:
            log.critical(
                f"Running as root! This may cause weird issues and problems with file ownership. Please re-run as a normal user! Or run: {sys.argv[0]} --allow-root ..."
            )
            exit(1)

    # Check prefix is ok
    if not re.fullmatch(prefix_format, args.nameprefix):
        log.critical(f'Invalid prefix: "{args.nameprefix}"')
        exit(1)

    if args.nameprefix != prefix_default:
        log.info(f"Using name prefix: {args.nameprefix}")

    # Pick selected command & run!
    selected = commands[args.command]
    selected.action_callback(args)
