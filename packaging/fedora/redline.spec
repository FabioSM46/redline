# Built by scripts/build-distro-package.sh from the checkout, not from a download.
%global debug_package %{nil}

Name:           redline
Version:        0.2.1
Release:        1%{?dist}
Summary:        Control panel for AMD GPUs: live stats, tuning, instant replay and recording
License:        MIT
URL:            https://github.com/FabioSM46/redline
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64

BuildRequires:  cargo gcc jq libayatana-appindicator-gtk3-devel librsvg2-devel libxdo-devel
BuildRequires:  nodejs openssl-devel perl-FindBin perl-File-Compare perl-IPC-Cmd pkgconf pnpm
BuildRequires:  python3 rust webkit2gtk4.1-devel
Requires:       hicolor-icon-theme libayatana-appindicator-gtk3 polkit
Recommends:     ffmpeg-free mangohud

%description
Watch temperatures, clocks and power draw, tune fans and power limits, and save a replay of
the last minute of gameplay. Redline coordinates LACT, GPU Screen Recorder and MangoHud.

%prep
%autosetup

%build
pnpm install --frozen-lockfile
python3 scripts/third-party-licenses.py --target x86_64-unknown-linux-gnu \
  --output src-tauri/target/THIRD-PARTY-LICENSES
pnpm tauri build --no-bundle

%install
bash packaging/install-files.sh "$PWD" %{buildroot}

%files
/usr/bin/redline
/usr/share/applications/com.fabiosm46.redline.desktop
/usr/share/icons/hicolor/*/apps/redline.png
/usr/share/licenses/redline/
/usr/share/polkit-1/actions/com.fabiosm46.redline.install.policy
/usr/share/redline/
