#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

"""Resolve bare-metal target IPs and publish them as job-scoped ADO variables.

Non-secret pipeline variables are available in the environment with uppercase
names and dots replaced by underscores. Parallel jobs use indexed IPs selected
by the agent name's trailing 1-based index; serial jobs use unindexed IPs.
"""

import argparse
import os
import re
import sys
from typing import Dict, Mapping


def resolve_target_ips(
    environment: Mapping[str, str], max_parallel: int
) -> Dict[str, str]:
    suffix = ""
    if max_parallel > 1:
        # Support AgentName as well as the predefined Agent.Name variable.
        agent_name = environment.get("AGENTNAME") or environment.get("AGENT_NAME", "")
        match = re.fullmatch(r".+ ([1-9][0-9]*)", agent_name)
        if not match:
            raise ValueError(
                "Parallel jobs require AgentName (or Agent.Name) in the form "
                "'<pool name> <1-based agent index>'"
            )
        suffix = f"_{match.group(1)}"

    addresses = {}
    for name in ("baremetal_controller.oam_ip", "baremetal_controller.bmc_ip"):
        source_name = f"{name}{suffix}"
        value = environment.get(source_name.upper().replace(".", "_"), "")
        if not value:
            raise ValueError(f"{source_name} is missing or empty")
        addresses[name] = value
    return addresses


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--max-parallel", type=int, required=True)
    args = parser.parse_args()

    try:
        addresses = resolve_target_ips(os.environ, args.max_parallel)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1

    for name, value in addresses.items():
        # Escape logging-command data so values cannot introduce extra commands.
        escaped = value.replace("%", "%AZP25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"##vso[task.setvariable variable={name}]{escaped}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
