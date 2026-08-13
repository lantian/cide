#!/bin/bash

cargo run -p xtask -- package --run
cp ./target/release/bundle/appimage/cide_*_amd64.AppImage /home/lantian/bin/cide_0.1.0_amd64.AppImage