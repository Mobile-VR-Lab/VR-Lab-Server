# VR Lab Server

This server's purpose is to balance the load of several dozen connections and to provide a friendly
API for the instructor app to interact with. This server takes some load off the instructor app,
so that both components can run on cheap or used hardware. This way, most of the money can be spent
on the nice VR headsets which do most of the heavy lifting.

## Development Details

See the wiki pages for more information.

## Installation

We offer a couple methods for installation. You may use:
- `cargo install` to install it as a binary on your user's path (requires a Rust toolchain installation)
- Using the provided Dockerfile and docker-compose.yml file to bring up a Docker Compose deployment.
- Using the install.sh script, which installs the server as a systemd service unit.

### Container installation

You can install the server as a docker container by using the provided Dockerfile.
This requires an OCI compliant builder runtime. It has been tested on Docker, and may also work with Podman and Buildah.

The following installation instructions assume you're using docker as your runtime.
1. Run `docker compose build` to build the container image.
2. Run `docker compose up` to bring the container up.
  - If you wish for the container to detach from your terminal, then pass the `-d` switch into the command for step 2.

### Service unit installation

The server may also be installed as a systemd service unit.
Simply run the `install.sh` script included, and follow all prompts. You may be prompted for your password, and this script requires that you have root access.

Once installed, you should see the `vr-lab-server.service` unit appear in systemd.
Note that the script installs and enables the new service unit such that it runs on startup, but it does not start it after the installation.

To start the service, run `systemctl start vr-lab-server.service`.
