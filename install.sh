#!/usr/bin/env bash
set -e

echo "================ VR Lab Server Installer ================"
printf "\n"
echo "This script requires cURL, Ubuntu 22.04 LTS or newer, and an internet connection. Other distros are untested, you may need to edit the service unit file."
echo "If you do not have NOPASSWD set, you will be prompted for the root password."
echo "For a non-root install, please build the Dockerfile with podman and use the container image."
echo "Or, use cargo to install, make sure you have your local cargo directory added to your PATH."
echo "If the Rust installer says you already have an installation of rustup, then please enter \"n\" and skip the rust installer."
printf "\n\n\n"
printf "Continue installing VR Lab Server? [y\N]: "

read RESULT

if [ $RESULT != 'y' ]; then
	echo "Exiting..."
	exit 0	
fi

echo "Continuing..."

sudo bash -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" || true
sudo bash -c 'source $HOME/.cargo/env \
	&& rustup install stable \
	&& cargo install --path . \
	&& cp vr-lab-server.service /etc/systemd/system/vr-lab-server.service \
	&& systemctl daemon-reload \
	&& systemctl enable vr-lab-server.service'

