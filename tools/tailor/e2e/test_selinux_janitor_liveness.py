#!/usr/bin/env python3
"""Synthetic liveness inputs; no loop device or mount is created or cleaned up."""

from pathlib import Path
import unittest
import uuid

from selinux_janitor_e2e import assert_no_live_references, assert_no_local_live_references


TARGET = Path("/disposable/work/artifacts/image.raw")


def mountinfo(root: str = "/", mountpoint: str = "/other", source: str = "/dev/vda") -> str:
    return f"42 1 8:1 {root} {mountpoint} rw - ext4 {source} rw\n"


class LivenessChecks(unittest.TestCase):
    def test_loop_backing_file_blocks_deletion_including_deleted_suffix(self) -> None:
        for backing in (str(TARGET), f"{TARGET} (deleted)"):
            with self.subTest(backing=backing), self.assertRaisesRegex(
                RuntimeError, "live loop"
            ):
                assert_no_live_references(TARGET, [("loop7", backing)], "")

    def test_mountpoint_root_and_source_block_deletion(self) -> None:
        for key, value in (
            ("mountpoint", str(TARGET)),
            ("root", str(TARGET)),
            ("source", str(TARGET)),
        ):
            with self.subTest(field=key), self.assertRaisesRegex(
                RuntimeError, "live mount"
            ):
                assert_no_live_references(TARGET, [], mountinfo(**{key: value}))

    def test_mountinfo_escaped_spaces_are_decoded(self) -> None:
        target = Path("/disposable/work/artifacts/my image.raw")
        with self.assertRaisesRegex(RuntimeError, "live mount"):
            assert_no_live_references(
                target, [], mountinfo(mountpoint="/disposable/work/artifacts/my\\040image.raw")
            )

    def test_unrelated_sibling_paths_do_not_block(self) -> None:
        assert_no_live_references(
            TARGET, [("loop7", f"{TARGET}-backup")],
            mountinfo(mountpoint=f"{TARGET}-backup"),
        )

    def test_malformed_mountinfo_fails_closed(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "malformed mountinfo"):
            assert_no_live_references(TARGET, [], "not a mountinfo entry")
        for escaped in (r"/some\777path", r"/some\04path", r"/some\q40path", r"/some\377path"):
            with self.subTest(escaped=escaped), self.assertRaisesRegex(
                RuntimeError, "malformed mountinfo"
            ):
                assert_no_live_references(TARGET, [], mountinfo(mountpoint=escaped))

    def test_file_reader_rejects_synthetic_sysfs_and_mountinfo(self) -> None:
        fixture = Path(__file__).resolve().parent / "artifacts" / f".liveness-{uuid.uuid4().hex}"
        loop_dir = fixture / "sys-block/loop7/loop"
        loop_dir.mkdir(parents=True)
        target = fixture / "image.raw"
        target.write_text("fixture")
        backing = loop_dir / "backing_file"
        mount_file = fixture / "mountinfo"
        try:
            backing.write_text(f"{target} (deleted)\n")
            mount_file.write_text(mountinfo())
            with self.assertRaisesRegex(RuntimeError, "live loop"):
                assert_no_local_live_references(target, fixture / "sys-block", mount_file)

            backing.write_text("")
            mount_file.write_text(mountinfo(source=str(target)))
            with self.assertRaisesRegex(RuntimeError, "live mount"):
                assert_no_local_live_references(target, fixture / "sys-block", mount_file)

            mount_file.write_text("")
            with self.assertRaisesRegex(AssertionError, "empty mountinfo"):
                assert_no_local_live_references(target, fixture / "sys-block", mount_file)
        finally:
            for path in (backing, mount_file, target):
                if path.exists():
                    path.unlink()
            for path in (loop_dir, loop_dir.parent, loop_dir.parent.parent, fixture):
                path.rmdir()


if __name__ == "__main__":
    unittest.main()
