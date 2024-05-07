#!/bin/python3

import re
import sys

if len(sys.argv) < 1:
    print("Usage: netlaunch_output.py <netlaunch-output-file>")
    sys.exit(1)

template = r"""
.*\n
.*Using\s+Trident\s+config\s+file: .+\n
.*Listening\.\.\.\s+address="\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}:\d{1,5}"\n
.*(")?ISO\s+deployed!(")?\s*\n
.*(")?Waiting\s+for\s+phone\s+home\.\.\.\s*(")?\n
(.*\n)*
.*(")?Trident\s+started\s+\(connection\s+attempt\s+\d+\)(")?\s+state=started\n
(.*\n)*
.*(")?Trident\s+started\s+\(connection\s+attempt\s+\d+\)(")?\s+state=started\n
(.*\n)*
.*(")?Host\s+Status:\\nspec:\\n.*\\n"\n
.*(")?provisioning\s+succeeded(")?\s+state=succeeded\s*
"""

successful_output = re.compile(template, re.MULTILINE | re.VERBOSE)

with open(sys.argv[1], "r") as netlaunch_output:
    output = netlaunch_output.read()
    match = successful_output.match(output)
    if match is not None:
        print("Successful Provisioning OS deployment, trident is running")
    else:
        raise Exception("There was an error durring deployment please check VM logs")
