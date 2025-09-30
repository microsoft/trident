from pathlib import Path
import os, errno
import contextlib
import tempfile


def main_location() -> str:
    import __main__

    return str(Path(__main__.__file__).absolute().parent)


def silentremove(filename):
    try:
        os.remove(filename)
    except OSError as e:
        if e.errno != errno.ENOENT:
            # Raise if the error is not related to the file existing or not
            raise


def read_file(file):
    try:
        with open(file, "r") as f:
            return f.read()
    except Exception as ex:
        return None


def default_ssh_key():
    return str(Path.home().joinpath(".ssh/id_rsa.pub").absolute())


def make_dir(path: str) -> str:
    pt = Path(path).absolute()
    pt.mkdir(parents=True, exist_ok=True)
    print(f"{pt} exists: {pt.exists()}")
    return str(pt)


def make_file(path: str) -> str:
    pt = Path(path).absolute()
    pt.parent.mkdir(parents=True, exist_ok=True)
    if pt.exists():
        if pt.is_file():
            return str(pt)
        else:
            raise ValueError("Invalid path: does not point to a file!")
    else:
        pt.touch()
        return str(pt)


@contextlib.contextmanager
def make_named_temp_file():
    file = tempfile.NamedTemporaryFile(delete=False)
    try:
        yield file
    finally:
        try:
            os.remove(file.name)
        except FileNotFoundError:
            pass
