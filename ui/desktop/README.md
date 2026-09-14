# goose Desktop App

Native desktop app for goose built with [Electron](https://www.electronjs.org/) and [ReactJS](https://react.dev/). 

# Building and running
goose uses [Hermit](https://github.com/cashapp/hermit) to manage dependencies, so you will need to have it installed and activated.

```
git clone git@github.com:aaif-goose/goose.git
cd goose
source ./bin/activate-hermit
cd ui/desktop
pnpm install
pnpm run start
```

## Platform-specific build requirements

### Linux
For building on Linux distributions, you'll need additional system dependencies:

**Debian/Ubuntu:**
```bash
sudo apt install dpkg fakeroot
```

**Arch/Manjaro:**
```bash
sudo pacman -S dpkg fakeroot
```

**Fedora/RHEL:**
```bash
sudo dnf install dpkg-dev fakeroot
```

# Building notes

This is an Electron Forge app using Vite and React. The desktop app launches the bundled `goose` CLI binary and talks to its ACP server.

## Building for different platforms

### macOS
`pnpm run bundle:default` will give you a goose.app/zip which is signed/notarized but only if you set up the env vars as per `forge.config.ts` (you can empty out the section on osxSign if you don't want to sign it) - this will have all defaults.

`pnpm run bundle:preconfigured` will make a goose.app/zip signed and notarized, but use the following:

```python
            f"        process.env.GOOSE_PROVIDER__TYPE = '{os.getenv("GOOSE_BUNDLE_TYPE")}';",
            f"        process.env.GOOSE_PROVIDER__HOST = '{os.getenv("GOOSE_BUNDLE_HOST")}';",
            f"        process.env.GOOSE_PROVIDER__MODEL = '{os.getenv("GOOSE_BUNDLE_MODEL")}';"
```

This allows you to set for example GOOSE_PROVIDER__TYPE to be "databricks" by default if you want (so when people start goose.app - they will get that out of the box). There is no way to set an api key in that bundling as that would be a terrible idea, so only use providers that can do oauth (like databricks can), otherwise stick to default goose.

### Linux
For Linux builds, first ensure you have the required system dependencies installed (see above), then:

1. Build the Rust binary:
```bash
cd ../..  # Go to project root
cargo build --release -p goose-cli --bin goose
```

2. Copy the binary to the expected location:
```bash
mkdir -p src/bin
cp ../../target/release/goose src/bin/
```

3. Build the application:
```bash
# For ZIP distribution (works on all Linux distributions)
pnpm run make --targets=@electron-forge/maker-zip --arch=x64

# For DEB package (Debian/Ubuntu)
pnpm run make --targets=@electron-forge/maker-deb --arch=x64

# For RPM package (Fedora/RHEL)
pnpm run make --targets=@electron-forge/maker-rpm --arch=x64

# For Flatpak (requires flatpak and flatpak-builder)
pnpm run make --targets=@electron-forge/maker-flatpak --arch=x64
```

The `--arch` option controls the Electron architecture only; it does not rebuild the Rust `goose` binary. To create an ARM64 package, run these steps on an ARM64 Linux host so `cargo build` produces an ARM64 `goose` binary, then replace `--arch=x64` with `--arch=arm64`. Do not package an ARM64 Electron application with the x64 `goose` binary produced on an x64 host.

Electron Forge writes packages to architecture-specific directories:

| Package | x64 | ARM64 |
| --- | --- | --- |
| ZIP | `out/make/zip/linux/x64/*.zip` | `out/make/zip/linux/arm64/*.zip` |
| DEB | `out/make/deb/x64/*_amd64.deb` | `out/make/deb/arm64/*_arm64.deb` |
| RPM | `out/make/rpm/x64/*.x86_64.rpm` | `out/make/rpm/arm64/*.arm64.rpm` |
| Flatpak | `out/make/flatpak/x86_64/*.flatpak` | `out/make/flatpak/aarch64/*.flatpak` |
| Application | `out/Goose-linux-x64/` | `out/Goose-linux-arm64/` |

### Windows
Use the existing Windows build process as documented.


# Running with an external ACP backend

From the project root, start the ACP backend:

```bash
GOOSE_SERVER__SECRET_KEY=test cargo run -p goose-cli --bin goose -- serve --platform desktop --enable-scheduler --host 127.0.0.1 --port 3000
```

Then start the desktop app from `ui/desktop`:

```bash
GOOSE_EXTERNAL_BACKEND=true GOOSE_EXTERNAL_BACKEND_URL=http://127.0.0.1:3000 GOOSE_SERVER__SECRET_KEY=test pnpm run start-gui
```
