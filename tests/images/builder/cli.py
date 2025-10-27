import argparse
from enum import Enum
import logging
from pathlib import Path

from typing import List

from builder import (
    ImageConfig,
    ArtifactManifest,
    SystemArchitecture,
    run,
)


logging.basicConfig(level=logging.INFO)
log = logging.getLogger("trident-testimages")


def positive_int(value) -> int:
    ivalue = int(value)
    if ivalue < 1:
        raise argparse.ArgumentTypeError(f"{value} is an invalid positive int value")
    return ivalue


class SubCommand(Enum):
    LIST = "list"
    DEPENDENCIES = "dependencies"
    BUILD = "build"
    LIST_FILES = "list-files"
    SHOW_ARTIFACT = "show-artifact"
    SHOW_IMAGE = "show-image"
    DOWNLOAD_IMAGE = "download-image"
    MATRIX = "matrix"


def init(configs: List[ImageConfig], artifacts: ArtifactManifest) -> None:
    parser = argparse.ArgumentParser(
        description="Get dependency list for specific image"
    )
    subparsers = parser.add_subparsers(
        help="Thing to do", dest="command", required=True, metavar="COMMAND"
    )

    setup_parser_list(subparsers)
    setup_parser_show(subparsers, artifacts)
    setup_subparser_dependencies(subparsers)
    setup_parser_build(subparsers, configs, artifacts)
    setup_parser_list_files(subparsers)
    setup_parser_download_image(subparsers, artifacts)
    setup_parser_matrix(subparsers)
    setup_parser_image_info(subparsers, configs)

    args = parser.parse_args()
    subcommand = SubCommand(args.command)

    run_cmd(configs, artifacts, subcommand, args)


def positive_int(value) -> int:
    ivalue = int(value)
    if ivalue < 1:
        raise argparse.ArgumentTypeError(f"{value} is an invalid positive int value")
    return ivalue


def setup_parser_list(subparsers: argparse._SubParsersAction) -> None:
    parser_list = subparsers.add_parser(
        SubCommand.LIST.value, help="List all image definitions"
    )


def setup_parser_show(
    subparsers: argparse._SubParsersAction,
    artifacts: ArtifactManifest,
) -> None:
    parser_show = subparsers.add_parser(
        SubCommand.SHOW_ARTIFACT.value,
        help="Show default artifact configuration.",
    )
    parser_show.set_defaults(artifacts=artifacts)
    parser_show.add_argument(
        "item",
        choices=ArtifactManifest.kebab_fields(),
        help="The item to show",
    )


def setup_subparser_dependencies(
    subparsers: argparse._SubParsersAction,
) -> None:
    parser_dependencies = subparsers.add_parser(
        SubCommand.DEPENDENCIES.value,
        help="List all dependencies for a specific image",
    )
    parser_dependencies.add_argument("image", help="The image to get dependencies for")


def setup_parser_build(
    subparsers: argparse._SubParsersAction,
    configs: List[ImageConfig],
    artifacts: ArtifactManifest,
) -> None:
    parser_build = subparsers.add_parser(
        SubCommand.BUILD.value, help="Build a specific image"
    )

    parser_build.set_defaults(artifacts=artifacts)

    parser_build.add_argument(
        "image", help="The image to build", choices=[c.name for c in configs]
    )
    parser_build.add_argument(
        "--output-dir",
        help="Where to write the output image.",
        default=Path.cwd() / "build",
        type=Path,
    )
    parser_build.add_argument(
        "--dry-run", action="store_true", help="Do not run the command"
    )
    parser_build.add_argument(
        "--container",
        default=artifacts.customizer_container_full,
        type=str,
        help="Configure Prism container image",
    )
    parser_build.add_argument(
        "--clones",
        type=positive_int,
        default=1,
        help="Number of clones of this image to create. "
        "The default is 1, which means no cloning will be done. When more than 1 is requested, "
        "the image file names will be suffixed with `_<number>` for each clone, starting with 0."
        " Clones are built in parallel, requesting multiple clones will become very resource "
        "intensive.",
    )
    parser_build.add_argument(
        "-f",
        "--force",
        action="store_true",
        help="Force the build even if the image already exists and it is up to date",
    )
    parser_build.add_argument(
        "--no-download",
        action="store_false",
        dest="download",
        help="By default, the builder will try to download any missing artifacts, "
        "this flag will disable that behavior.",
    )


def setup_parser_list_files(
    subparsers: argparse._SubParsersAction,
) -> None:
    parser_targets = subparsers.add_parser(
        SubCommand.LIST_FILES.value, help="List all images as file targets"
    )
    parser_targets.add_argument(
        "--output-dir",
        help="Where to write the output image.",
        default=Path("build"),
        type=Path,
    )


def setup_parser_download_image(
    subparsers: argparse._SubParsersAction,
    artifacts: ArtifactManifest,
) -> None:
    parser_download_img = subparsers.add_parser(
        SubCommand.DOWNLOAD_IMAGE.value,
        help="Download a base image from the Azure DevOps feed",
    )
    parser_download_img.set_defaults(artifacts=artifacts)
    parser_download_img.add_argument(
        "image",
        help="The image to download",
        choices=[c.image.name for c in artifacts.base_images],
    )


def setup_parser_matrix(
    subparsers: argparse._SubParsersAction,
) -> None:
    parser_matrix = subparsers.add_parser(
        SubCommand.MATRIX.value,
        help="Generate ADO Pipeline matrix for all images",
    )
    parser_matrix.add_argument(
        "-a",
        "--arch",
        help=f"Architecture to build for. '{SystemArchitecture.AMD64.value}' or "
        f"'{SystemArchitecture.ARM64.value}'",
        default=SystemArchitecture.AMD64.value,
        type=SystemArchitecture,
    )
    parser_matrix.add_argument(
        "--indent",
        help="Indentation for the matrix",
        default=None,
        type=int,
    )


def setup_parser_image_info(
    subparsers: argparse._SubParsersAction,
    configs: List[ImageConfig],
) -> None:
    parser_image_info = subparsers.add_parser(
        SubCommand.SHOW_IMAGE.value,
        help="Show image information",
    )
    parser_image_info.add_argument(
        "image",
        help="The image to show",
        choices=[c.name for c in configs],
    )
    parser_image_info.add_argument(
        "field",
        help="The field to show",
        choices=ImageConfig.kebab_fields(),
    )
    parser_image_info.add_argument(
        "--devops-var",
        help="Output the field as a DevOps variable with the given name",
        default=None,
        type=str,
    )


def run_cmd(
    configs: List[ImageConfig],
    artifacts: ArtifactManifest,
    subcommand: SubCommand,
    args: argparse.Namespace,
):
    if subcommand == SubCommand.LIST:
        run.list_configs(
            configs=configs,
        )
    elif subcommand == SubCommand.DEPENDENCIES:
        run.list_dependencies(
            configs=configs,
            name=args.image,
        )
    elif subcommand == SubCommand.BUILD:
        run.build(
            artifacts=args.artifacts,
            configs=configs,
            name=args.image,
            container_name=args.container,
            output_dir=args.output_dir,
            clones=args.clones,
            dry_run=args.dry_run,
            force=args.force,
            download=args.download,
        )
    elif subcommand == SubCommand.LIST_FILES:
        run.list_files(
            configs=configs,
            output_dir=args.output_dir,
        )
    elif subcommand == SubCommand.SHOW_ARTIFACT:
        run.show_artifact(
            artifacts=args.artifacts,
            item=args.item,
        )
    elif subcommand == SubCommand.DOWNLOAD_IMAGE:
        run.download_base_image(
            artifacts=args.artifacts,
            name=args.image,
        )
    elif subcommand == SubCommand.MATRIX:
        run.generate_matrix(
            configs=configs,
            arch=args.arch,
            indent=args.indent,
        )
    elif subcommand == SubCommand.SHOW_IMAGE:
        run.show_image(
            configs=configs,
            name=args.image,
            field_name=args.field,
            devops_var=args.devops_var,
        )
    else:
        raise ValueError(f"Unknown subcommand: {subcommand}")
