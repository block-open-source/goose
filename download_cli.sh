#!/usr/bin/env bash
set -eu

##############################################################################
# goose CLI Install Script
#
# This script downloads the latest stable 'goose' CLI binary from GitHub releases
# and installs it to your system.
#
# Supported OS: macOS (darwin), Linux, Windows (MSYS2/Git Bash/WSL), Android (Termux)
# Supported Architectures: x86_64, arm64
#
# Usage:
#   curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | bash
#
# Environment variables:
#   GOOSE_BIN_DIR  - Directory to which goose will be installed (default: $HOME/.local/bin)
#   GOOSE_VERSION  - Optional: specific version to install (e.g., "v1.0.25"). Overrides CANARY. Can be in the format vX.Y.Z, vX.Y.Z-suffix, or X.Y.Z
#   GOOSE_PROVIDER - Optional: provider for goose
#   GOOSE_MODEL    - Optional: model for goose
#   GOOSE_LINUX_VARIANT - Optional: Linux package variant to install (`standard`, `vulkan`, or `musl`)
#   GOOSE_WINDOWS_VARIANT - Optional: Windows package variant to install (`standard` or `cuda`)
#   CANARY         - Optional: if set to "true", downloads from canary release instead of stable
#   CONFIGURE      - Optional: if set to "false", disables running goose configure interactively
#   ** other provider specific environment variables (eg. DATABRICKS_HOST)
##############################################################################

# --- 1) Check for dependencies ---
# Check for curl
if ! command -v curl >/dev/null 2>&1; then
  echo "Error: 'curl' is required to download goose. Please install curl and try again."
  exit 1
fi

# Check for tar or unzip (depending on OS)
if ! command -v tar >/dev/null 2>&1 && ! command -v unzip >/dev/null 2>&1; then
  echo "Error: Either 'tar' or 'unzip' is required to extract goose. Please install one and try again."
  exit 1
fi

# Check for required extraction tools based on detected OS
if [ "${OS:-}" = "windows" ]; then
  # Windows uses PowerShell's built-in Expand-Archive - check if PowerShell is available
  if ! command -v powershell.exe >/dev/null 2>&1 && ! command -v pwsh >/dev/null 2>&1; then
    echo "Warning: PowerShell is recommended to extract Windows packages but was not found."
    echo "Falling back to unzip if available."
  fi
else
  if ! command -v tar >/dev/null 2>&1; then
    echo "Error: 'tar' is required to extract packages for ${OS:-unknown}. Please install tar and try again."
    exit 1
  fi
fi


# --- 2) Variables ---
REPO="aaif-goose/goose"
OUT_FILE="goose"

# Set default bin directory based on detected OS environment
if [[ "${WINDIR:-}" ]] || [[ "${windir:-}" ]] || [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then
    # Native Windows environments - use Windows user profile path
    DEFAULT_BIN_DIR="$USERPROFILE/goose"
else
    # Linux, macOS, and WSL all use the same bin directory
    DEFAULT_BIN_DIR="$HOME/.local/bin"
fi

GOOSE_BIN_DIR="${GOOSE_BIN_DIR:-$DEFAULT_BIN_DIR}"
RELEASE="${CANARY:-false}"
CONFIGURE="${CONFIGURE:-true}"
GOOSE_LINUX_VARIANT="${GOOSE_LINUX_VARIANT:-}"
GOOSE_WINDOWS_VARIANT="${GOOSE_WINDOWS_VARIANT:-standard}"
if [ -n "${GOOSE_VERSION:-}" ]; then
  # Validate the version format
  if [[ ! "$GOOSE_VERSION" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-.*)?$ ]]; then
    echo "[error]: invalid version '$GOOSE_VERSION'."
    echo "  expected: semver format vX.Y.Z, vX.Y.Z-suffix, or X.Y.Z"
    exit 1
  fi
  GOOSE_VERSION=$(echo "$GOOSE_VERSION" | sed 's/^v\{0,1\}/v/') # Ensure the version string is prefixed with 'v' if not already present
  RELEASE_TAG="$GOOSE_VERSION"
else
  # If GOOSE_VERSION is not set, fall back to existing behavior for backwards compatibility
  RELEASE_TAG="$([[ "$RELEASE" == "true" ]] && echo "canary" || echo "stable")"
fi

# --- 3) Detect OS/Architecture ---
# Allow explicit override for automation or when auto-detection is wrong:
#   INSTALL_OS=linux|windows|darwin
if [ -n "${INSTALL_OS:-}" ]; then
  case "${INSTALL_OS}" in
    linux|windows|darwin) OS="${INSTALL_OS}" ;;
    *) echo "[error]: unsupported INSTALL_OS='${INSTALL_OS}' (expected: linux|windows|darwin)"; exit 1 ;;
  esac
else
  # Better OS detection for Windows environments, with safer WSL handling.
  # If explicit Windows-like shells/variables are present (MSYS/Cygwin), treat as windows.
  if [[ "${WINDIR:-}" ]] || [[ "${windir:-}" ]] || [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then
    OS="windows"
  elif [[ -n "${TERMUX_VERSION:-}" ]]; then
    # Termux on Android: treat as Linux before the Windows mount heuristic,
    # since /d may exist on Android and would incorrectly match as Windows.
    OS="linux"
  elif [[ -f "/proc/version" ]] && grep -q "Microsoft\|WSL" /proc/version 2>/dev/null; then
    # WSL is a Linux environment regardless of the current working directory.
    # The PWD (e.g. /mnt/c/) does not change the kernel — always install Linux.
    OS="linux"
  elif [[ "$OSTYPE" == "darwin"* ]]; then
    OS="darwin"
  elif [[ "$PWD" =~ ^/[a-zA-Z]/ ]] && [[ -d "/c" || -d "/d" || -d "/e" ]]; then
    # Check for Windows-style mount points (like in Git Bash)
    OS="windows"
  else
    # Fallback to uname for other systems
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  fi
fi

ARCH=$(uname -m)

# Handle Windows environments (MSYS2, Git Bash, Cygwin, WSL)
case "$OS" in
  linux|darwin|windows) ;;
  mingw*|msys*|cygwin*)
    OS="windows"
    ;;
  *)
    echo "Error: Unsupported OS '$OS'. goose currently supports Linux, macOS, and Windows."
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64)
    ARCH="x86_64"
    ;;
  arm64|aarch64)
    # Some systems use 'arm64' and some 'aarch64' – standardize to 'aarch64'
    ARCH="aarch64"
    ;;
  *)
    echo "Error: Unsupported architecture '$ARCH'."
    exit 1
    ;;
esac

detect_linux_musl() {
  if [[ "$OSTYPE" == "linux-musl"* ]]; then
    return 0
  fi

  if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    return 0
  fi

  return 1
}

# Termux on Android: the musl portable build is the best fit (no system-keyring, no local-inference).
if [ "$OS" = "linux" ] && [ -n "${TERMUX_VERSION:-}" ] && [ -z "$GOOSE_LINUX_VARIANT" ]; then
  echo "Termux detected (v$TERMUX_VERSION). Using musl portable build."
  GOOSE_LINUX_VARIANT="musl"
fi

if [ "$OS" = "linux" ] && [ -z "$GOOSE_LINUX_VARIANT" ]; then
  if detect_linux_musl; then
    GOOSE_LINUX_VARIANT="musl"
  else
    GOOSE_LINUX_VARIANT="standard"
  fi
elif [ -z "$GOOSE_LINUX_VARIANT" ]; then
  GOOSE_LINUX_VARIANT="standard"
fi

# Debug output (safely handle undefined variables)
echo "WINDIR: ${WINDIR:-<not set>}"
echo "OSTYPE: $OSTYPE"
echo "uname -s: $(uname -s)"
echo "uname -m: $(uname -m)"
echo "PWD: $PWD"

# Output the detected OS
echo "Detected OS: $OS with ARCH $ARCH"

# Build the filename and URL for the stable release
if [ "$OS" = "darwin" ]; then
  FILE="goose-$ARCH-apple-darwin.tar.bz2"
  EXTRACT_CMD="tar"
elif [ "$OS" = "windows" ]; then
  case "$GOOSE_WINDOWS_VARIANT" in
    standard|cuda) ;;
    *)
      echo "Error: Unsupported GOOSE_WINDOWS_VARIANT '$GOOSE_WINDOWS_VARIANT'. Expected 'standard' or 'cuda'."
      exit 1
      ;;
  esac
  # Windows only supports x86_64 currently
  if [ "$ARCH" != "x86_64" ]; then
    echo "Error: Windows currently only supports x86_64 architecture."
    exit 1
  fi
  FILE="goose-$ARCH-pc-windows-msvc.zip"
  if [ "$GOOSE_WINDOWS_VARIANT" = "cuda" ]; then
    FILE="goose-$ARCH-pc-windows-msvc-cuda.zip"
  fi
  EXTRACT_CMD="unzip"
  OUT_FILE="goose.exe"
else
  case "$GOOSE_LINUX_VARIANT" in
    standard|vulkan|musl) ;;
    *)
      echo "Error: Unsupported GOOSE_LINUX_VARIANT '$GOOSE_LINUX_VARIANT'. Expected 'standard', 'vulkan', or 'musl'."
      exit 1
      ;;
  esac
  FILE="goose-$ARCH-unknown-linux-gnu.tar.bz2"
  if [ "$GOOSE_LINUX_VARIANT" = "vulkan" ]; then
    FILE="goose-$ARCH-unknown-linux-gnu-vulkan.tar.bz2"
  elif [ "$GOOSE_LINUX_VARIANT" = "musl" ]; then
    FILE="goose-$ARCH-unknown-linux-musl.tar.bz2"
  fi
  EXTRACT_CMD="tar"
fi

DOWNLOAD_URL="https://github.com/$REPO/releases/download/$RELEASE_TAG/$FILE"

# --- 4) Download & extract 'goose' binary ---
echo "Downloading $RELEASE_TAG release: $FILE..."
if ! curl -sLf "$DOWNLOAD_URL" --output "$FILE"; then
  # If the download fails, only fall back to latest stable when no version was specified and canary was not requested).
  if ! [ -n "${GOOSE_VERSION:-}" ] && [ "${CANARY:-false}" != "true" ]; then
    LATEST_TAG=$(curl -s https://api.github.com/repos/aaif-goose/goose/releases/latest | \
      grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    if [ -z "$LATEST_TAG" ]; then
      echo "Error: Failed to download $DOWNLOAD_URL and latest tag unavailable"
      exit 1
    fi

    DOWNLOAD_URL="https://github.com/$REPO/releases/download/$LATEST_TAG/$FILE"
    if curl -sLf "$DOWNLOAD_URL" --output "$FILE"; then
      # Fallback succeeded
      :
    else
      echo "Error: Failed to download from fallback url $DOWNLOAD_URL using latest tag $LATEST_TAG"
      exit 1
    fi
  else
    echo "Error: Failed to download $DOWNLOAD_URL"
    exit 1
  fi
fi

# Create a temporary directory for extraction
TMP_DIR="${TMPDIR:-/tmp}/goose_install_$RANDOM"
if ! mkdir -p "$TMP_DIR"; then
  echo "Error: Could not create temporary extraction directory"
  exit 1
fi
# Clean up temporary directory
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Extracting $FILE to temporary directory..."
set +e  # Disable immediate exit on error

if [ "$EXTRACT_CMD" = "tar" ]; then
  tar -xjf "$FILE" -C "$TMP_DIR" 2> tar_error.log
  extract_exit_code=$?

  # Check for tar errors
  if [ $extract_exit_code -ne 0 ]; then
    if grep -iEq "missing.*bzip2|bzip2.*missing|bzip2.*No such file|No such file.*bzip2" tar_error.log; then
      echo "Error: Failed to extract $FILE. 'bzip2' is required but not installed. See details below:"
    else
      echo "Error: Failed to extract $FILE. See details below:"
    fi
    cat tar_error.log
    rm tar_error.log
    exit 1
  fi
  rm tar_error.log
else
  # Use unzip for Windows
  unzip -q "$FILE" -d "$TMP_DIR" 2> unzip_error.log
  extract_exit_code=$?

  # Check for unzip errors
  if [ $extract_exit_code -ne 0 ]; then
    echo "Error: Failed to extract $FILE. See details below:"
    cat unzip_error.log
    rm unzip_error.log
    exit 1
  fi
  rm unzip_error.log
fi

set -e  # Re-enable immediate exit on error

rm "$FILE" # clean up the downloaded archive

# Determine the extraction directory (handle subdirectory in Windows packages)
# Windows releases may contain files in a 'goose-package' subdirectory
EXTRACT_DIR="$TMP_DIR"
if [ "$OS" = "windows" ] && [ -d "$TMP_DIR/goose-package" ]; then
  echo "Found goose-package subdirectory, using that as extraction directory"
  EXTRACT_DIR="$TMP_DIR/goose-package"
fi

# Make binary executable
if [ "$OS" = "windows" ]; then
  chmod +x "$EXTRACT_DIR/goose.exe"
else
  chmod +x "$EXTRACT_DIR/goose"
fi

# --- 5) Install to $GOOSE_BIN_DIR ---
if [ ! -d "$GOOSE_BIN_DIR" ]; then
  echo "Creating directory: $GOOSE_BIN_DIR"
  mkdir -p "$GOOSE_BIN_DIR"
fi

echo "Moving goose to $GOOSE_BIN_DIR/$OUT_FILE"
if [ "$OS" = "windows" ]; then
  mv "$EXTRACT_DIR/goose.exe" "$GOOSE_BIN_DIR/$OUT_FILE"
else
  # On Linux, if the target binary is currently running, writing to it fails
  # with ETXTBSY ("Text file busy"). Rename the old binary out of the way
  # first, then move the new one in. If the move fails, restore the old binary
  # so the user is never left without an executable.
  if [ -f "$GOOSE_BIN_DIR/$OUT_FILE" ]; then
    mv "$GOOSE_BIN_DIR/$OUT_FILE" "$GOOSE_BIN_DIR/$OUT_FILE.old"
    if ! mv "$EXTRACT_DIR/goose" "$GOOSE_BIN_DIR/$OUT_FILE"; then
      echo "Error: failed to install new binary, restoring previous version"
      mv "$GOOSE_BIN_DIR/$OUT_FILE.old" "$GOOSE_BIN_DIR/$OUT_FILE"
      exit 1
    fi
    rm -f "$GOOSE_BIN_DIR/$OUT_FILE.old"
  else
    mv "$EXTRACT_DIR/goose" "$GOOSE_BIN_DIR/$OUT_FILE"
  fi
fi

# Copy Windows runtime DLLs if they exist
if [ "$OS" = "windows" ]; then
  for dll in "$EXTRACT_DIR"/*.dll; do
    if [ -f "$dll" ]; then
      echo "Moving Windows runtime DLL: $(basename "$dll")"
      mv "$dll" "$GOOSE_BIN_DIR/"
    fi
  done
fi

# skip configuration for non-interactive installs e.g. automation, docker
if [ "$CONFIGURE" = true ]; then
  # --- 6) Configure goose (Optional) ---
  echo ""
  echo "Configuring goose"
  echo ""
  if [ -t 0 ]; then
    "$GOOSE_BIN_DIR/$OUT_FILE" configure
  elif [ -r /dev/tty ]; then
    "$GOOSE_BIN_DIR/$OUT_FILE" configure < /dev/tty
  else
    echo "Non-interactive shell detected (e.g. 'curl ... | bash')."
    echo "Skipping 'goose configure' — please run it manually after installation:"
    echo "    $GOOSE_BIN_DIR/$OUT_FILE configure"
  fi
else
  echo "Skipping 'goose configure', you may need to run this manually later"
fi



# --- 7) Check PATH and give instructions if needed ---
if [[ ":$PATH:" != *":$GOOSE_BIN_DIR:"* ]]; then
  echo ""
  echo "Warning: goose installed, but $GOOSE_BIN_DIR is not in your PATH."

  if [ "$OS" = "windows" ]; then
    echo "To add goose to your PATH in PowerShell:"
    echo ""
    echo "# Add to your PowerShell profile"
    echo '$profilePath = $PROFILE'
    echo 'if (!(Test-Path $profilePath)) { New-Item -Path $profilePath -ItemType File -Force }'
    echo 'Add-Content -Path $profilePath -Value ''$env:PATH = "$env:USERPROFILE\.local\bin;$env:PATH"'''
    echo "# Reload profile or restart PowerShell"
    echo '. $PROFILE'
    echo ""
    echo "Alternatively, you can run:"
    echo "    goose configure"
    echo "or rerun this install script after updating your PATH."
  else
    SHELL_NAME=$(basename "$SHELL")

    # Appends an export line to a file only if that file does not already
    # put $GOOSE_BIN_DIR on the PATH, so re-running the installer is idempotent.
    add_path_line() {
      file="$1"
      line="$2"
      mkdir -p "$(dirname "$file")"
      if [ -f "$file" ] && grep -Fq "$GOOSE_BIN_DIR" "$file"; then
        echo "$file already references $GOOSE_BIN_DIR, skipping."
      else
        echo "$line" >> "$file"
        echo "Added \$GOOSE_BIN_DIR to $file"
      fi
    }

    # Pick the file(s) to update for the user's login shell.
    # POSIX_LOGIN_FILE is read by login shells, desktop launchers, ssh, cron;
    # RC_FILE (when set) is read by interactive shells.
    EXPORT_LINE="export PATH=\"$GOOSE_BIN_DIR:\$PATH\""
    case "$SHELL_NAME" in
    bash)
      # An existing ~/.bash_profile suppresses ~/.profile for bash login shells.
      if [ -f "$HOME/.bash_profile" ]; then
        POSIX_LOGIN_FILE="$HOME/.bash_profile"
      else
        POSIX_LOGIN_FILE="$HOME/.profile"
      fi
      RC_FILE="$HOME/.bashrc"
      ;;
    zsh)
      POSIX_LOGIN_FILE=""
      RC_FILE="$HOME/.zshrc"
      ;;
    fish)
      POSIX_LOGIN_FILE=""
      RC_FILE="$HOME/.config/fish/config.fish"
      EXPORT_LINE="fish_add_path \"$GOOSE_BIN_DIR\""
      ;;
    *)
      # sh, dash, and anything else POSIX-ish: ~/.profile is the standard file.
      POSIX_LOGIN_FILE="$HOME/.profile"
      RC_FILE=""
      ;;
    esac

    echo ""
    echo "The \$GOOSE_BIN_DIR is not in your PATH."

    if [ "$CONFIGURE" = true ]; then
      echo "What would you like to do?"
      echo "1) Add it for me"
      echo "2) I'll add it myself, show instructions"

      # Check whether stdin is a terminal. If it is not (for example, if
      # this script has been piped into bash), we need to explicitly read user's
      # choice from /dev/tty.
      if [ -t 0 ]; then # terminal
        read -p "Enter choice [1/2]: " choice
      elif [ -r /dev/tty ]; then # not a terminal, but /dev/tty is available
        read -p "Enter choice [1/2]: " choice < /dev/tty
      else # non-interactive environment without /dev/tty
        echo "Non-interactive environment detected without /dev/tty; defaulting to option 2 (show instructions)."
        choice=2
      fi

      case "$choice" in
      1)
        if [ -n "$POSIX_LOGIN_FILE" ]; then
          add_path_line "$POSIX_LOGIN_FILE" "$EXPORT_LINE"
        fi
        if [ -n "$RC_FILE" ]; then
          add_path_line "$RC_FILE" "$EXPORT_LINE"
        fi
        echo "Done! Start a new shell or log in again to apply changes."
        ;;
      2)
        echo ""
        echo "Add it to your PATH by adding this line to ${POSIX_LOGIN_FILE:-$RC_FILE}:"
        echo "    $EXPORT_LINE"
        echo "Then start a new shell or log in again to apply changes."
        ;;
      *)
        echo "Invalid choice. Please add \$GOOSE_BIN_DIR to your PATH manually."
        ;;
      esac
    else
      echo ""
      echo "Configure disabled. Please add \$GOOSE_BIN_DIR to your PATH manually."
    fi

  fi

  echo ""
fi
