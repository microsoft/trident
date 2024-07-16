#!/usr/bin/python3

import argparse
import os
import time
import re
from contextlib import contextmanager

parser = argparse.ArgumentParser(
    description="Waits for a login prompt on a VM's serial console."
)

parser.add_argument(
    "-d",
    "--device",
    type=str,
    required=True,
    help="Path to the serial device file.",
)

parser.add_argument(
    "-t",
    "--timeout",
    type=int,
    default=30,
    help="Timeout in seconds.",
)

parser.add_argument(
    "-o",
    "--output",
    type=str,
    default=None,
    help="Path to save the output.",
)

args = parser.parse_args()

# ANSI escape code cleaner
ansi_cleaner = re.compile(r"(\x9B|\x1B\[)[0-?]*[ -\/]*[@-~]")

# ANSI non-color escape code cleaner, matches only control codes
ansi_control_cleaner = re.compile(r"(\x9B|\x1B\[)[0-?]*[ -\/]*[@-ln-~]")


@contextmanager
def timeout(seconds: int):
    start_time = time.perf_counter()
    yield lambda: (time.perf_counter() - start_time) > seconds


# Remove the output file if it exists
if args.output and os.path.exists(args.output):
    os.remove(args.output)


def print_and_save(line: str):
    if not line:
        return
    # Remove ANSI control codes
    line = ansi_control_cleaner.sub("", line)
    print(line)
    if args.output:
        # Remove all ANSI escape codes
        line = ansi_cleaner.sub("", line)
        with open(args.output, "a") as output:
            output.write(line + "\n")


# Wait for the login prompt
with timeout(args.timeout) as is_timed_out:
    # Open the serial device file
    with open(args.device, "r", encoding="utf-8", errors="replace") as serial:
        # Read the serial output until the login prompt is found, or until the
        # timeout is reached
        line_buffer = ""
        while not is_timed_out():
            char = serial.read(1)
            if not char:
                continue

            # Check if the current line contains the login prompt
            if "login:" in line_buffer and "mos" not in line_buffer:
                print_and_save(line_buffer)
                break

            # If the last character is a newline, print the line buffer
            # and reset it
            if char == "\n":
                print_and_save(line_buffer)
                line_buffer = ""
            else:
                # Append the output to the buffer
                line_buffer += char
        else:
            # Raise a TimeoutError if the login prompt was not found
            raise TimeoutError("Timeout while waiting for login prompt.")
