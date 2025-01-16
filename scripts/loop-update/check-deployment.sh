#!/bin/bash
set -euxo pipefail

. $(dirname $0)/common.sh

VM_IP=`getIp`
checkActiveVolume "volume-a"