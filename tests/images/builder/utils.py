from pathlib import Path

BUILD_DIR = "/tmp"
HOST_PATH = Path("/host")


def build_path(path: Path) -> Path:
    """
    Convert a host filesystem path to its corresponding path inside the Docker container.

    The Docker container mounts the host's root filesystem at /host, so this function
    transforms absolute host paths like '/home/user/file.txt' to container paths
    like '/host/home/user/file.txt'.

    Args:
        path: Absolute or relative path on the host filesystem

    Returns:
        Path that can be used inside the Docker container to access the same file

    Example:
        build_path(Path("/home/user/config.yaml")) -> Path("/host/home/user/config.yaml")
    """
    return HOST_PATH / path.absolute().relative_to(Path("/"))
