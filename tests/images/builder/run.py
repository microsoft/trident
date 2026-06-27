import logging
from pathlib import Path
import json
from typing import List, Optional

from builder import ArtifactManifest, BlobImageManifest, ImageConfig, RpmSources
from .builder import build_image
from .convert import convert_image
from . import download

log = logging.getLogger(__name__)


def find_image(configs: List[ImageConfig], name: str) -> ImageConfig:
    for config in configs:
        if config.name == name:
            return config
    raise ValueError(f"Image '{name}' is not defined")


def list_configs(
    *, configs: List[ImageConfig], filter_type: Optional[str] = None
) -> None:
    for config in configs:
        if filter_type is None:
            print(config.name)

        for output_format in config.output_and_config:
            if filter_type == output_format.ext():
                print(config.name)


def list_files(*, configs: List[ImageConfig], output_dir: Path) -> None:
    for config in configs:
        print(output_dir / config.file_name())


def list_dependencies(*, configs: List[ImageConfig], name: str) -> None:
    image = find_image(configs, name)
    for dep in image.dependencies():
        print(dep)


def show_artifact(*, artifacts: ArtifactManifest, item: str) -> None:
    item = getattr(artifacts, item.replace("-", "_"))
    if isinstance(item, str):
        print(item)
    elif isinstance(item, list):
        for i in item:
            print(i)
    else:
        raise ValueError(f"Unknown item type: {type(item)}")


def show_image(
    *,
    configs: List[ImageConfig],
    name: str,
    field_name: str,
    devops_var: Optional[str] = None,
) -> None:
    image = find_image(configs, name)
    field = getattr(image, field_name.replace("-", "_"), None)

    out: str = None

    if field is None:
        raise ValueError(f"Field '{field_name}' not found in image '{name}'")
    if isinstance(field, str):
        out = field
    elif isinstance(field, list):
        out = "\n".join([str(i) for i in field])
    elif isinstance(field, dict):
        out = "\n".join(f"{getattr(k, 'value', k)}: {v}" for k, v in field.items())
    elif hasattr(field, "__str__") and callable(field.__str__):
        out = str(field)
    else:
        raise ValueError(f"Unknown field type: {type(field)}")

    if devops_var:
        print(f"##vso[task.setvariable variable={devops_var}]{out}")
    else:
        print(out)


def build(
    *,
    artifacts: ArtifactManifest,
    configs: List[ImageConfig],
    name: str,
    output_type: Optional[str],
    container_name: str,
    output_dir: Path,
    clones: int,
    dry_run: bool,
    force: bool,
    image_architecture: Optional[str] = None,
    download: bool = True,
) -> None:
    image = find_image(configs, name)
    log.info(f"Building image '{image.name}'")

    if output_type is not None:
        image.set_output_type(output_type)
        log.info(f"Building image with output type '{image.output_format().ic_name()}'")
    else:
        log.info(
            f"Building image with default output type '{image.output_format().ic_name()}'"
        )

    container_image: Optional[str] = container_name
    if container_image is None:
        log.error("Image Customizer container image is required")
        exit(1)

    if image.image_customizer_convert:
        # If 'convert' is requested, run Image Customizer convert subcommand
        convert_image(
            container_image,
            image.id,
            image.base_image.path,
            image.output_format().ic_name(),
            output_dir / image.file_name(),
            image_architecture,
            dry_run,
        )
    else:
        rpm_sources: List[Path] = []
        if image.requires_trident:
            rpm_sources.append(RpmSources.TRIDENT.path())
        if image.requires_dhcp:
            rpm_sources.append(RpmSources.DHCP.path())

        rpm_overrides_path = RpmSources.RPM_OVERRIDES.path()
        if rpm_overrides_path.exists():
            rpm_sources.append(rpm_overrides_path)

        if not image.base_image.path.exists():
            if download:
                log.info(
                    f"Downloading base image to '{image.base_image.path}'"
                    " (use --no-download to skip this step)"
                )

            else:
                log.error(f"Base image '{image.base_image.path}' does not exist.")
                exit(1)

        build_image(
            container_image=container_image,
            image=image,
            output_dir=output_dir,
            artifacts=artifacts,
            clones=clones,
            rpm_sources=rpm_sources,
            image_architecture=image_architecture,
            dry_run=dry_run,
            force=force,
        )


def download_base_image(
    *,
    artifacts: ArtifactManifest,
    name: str,
    blob_storage_account: Optional[str] = None,
    blob_container: Optional[str] = None,
) -> None:
    image_manifest = next(
        (img for img in artifacts.base_images if img.image.name == name), None
    )
    if image_manifest is None:
        raise ValueError(f"Image '{name}' not found in artifacts")
    log.info(f"Downloading base image '{name}' to '{image_manifest.image.path}'")

    if isinstance(image_manifest, BlobImageManifest):
        download.download_blob_image(
            image_manifest,
            storage_account=blob_storage_account,
            container=blob_container,
        )
    else:
        download.download_base_image(image_manifest)


def generate_matrix(
    *,
    configs: List[ImageConfig],
    arch: str,
    indent: Optional[int] = None,
) -> None:
    matrix = {}
    for config in configs:
        if config.architecture == arch:
            matrix[config.name] = {
                "image_name": config.name,
                "base_image": config.base_image.name,
                "img_file": str(config.base_image.path),
            }
    print(json.dumps(matrix, indent=indent))
