#!/bin/bash
set -eo pipefail

default_docker_id="jevolk/tuwunel"
docker_id=${docker_id:=$default_docker_id}
docker_acct=${docker_acct:=$(echo $docker_id | cut -d"/" -f1)}
docker_repo=${docker_repo:=$(echo $docker_id | cut -d"/" -f2)}

CI="${CI:-true}"
BASEDIR=$(dirname "$0")

set -a
runner_name=$(echo $RUNNER_NAME | cut -d"." -f1)
runner_num=$(echo $RUNNER_NAME | cut -d"." -f2)
set +a

###############################################################################

tester_image="complement-tester--${feat_set}--${sys_name}--${sys_version}--${sys_target}"
testee_image="complement-testee--${cargo_profile}--${rust_toolchain}--${rust_target}--${feat_set}--${sys_name}--${sys_version}--${sys_target}"
name="complement_tester__${cargo_profile}__${rust_toolchain}__${rust_target}__${feat_set}__${sys_name}__${sys_version}__${sys_target}"
sock="/var/run/docker.sock"
arg="--rm --name $name -v $sock:$sock --network=host $tester_image ${testee_image}"

trap 'set +x; date; echo -e "\033[1;41;37mFAIL\033[0m"' ERR
date
env
set -x -e
cid=$(docker run -d $arg)
set +x
trap 'docker container stop $cid; set +x; date; echo -e "\033[1;41;37mFAIL\033[0m"' INT
docker wait "$cid" 2>/dev/null
echo -e "\033[1;42;37mPASS\033[0m"
