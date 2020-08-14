#!/bin/bash

set -e;
set -x;

curl https://sh.rustup.rs | sh -s -- -y;
export PATH="$PATH:~/.cargo/bin/";
rustup set profile minimal;
rustup toolchain install nightly;
rustup override set nightly;
cd ./pylancelot;
maturin --release --strip;
cd ../;
ls -R target;
