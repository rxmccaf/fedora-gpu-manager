Name:           fedora-gpu-manager
Version:        1.1.1
Release:        1%{?dist}
Summary:        GPU driver detection and management tool for Fedora

License:        MIT
URL:            https://github.com/rxmccaf/fedora-gpu-manager
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gcc

Requires:       gtk4
Requires:       libadwaita

%description
fedora-gpu-manager detects NVIDIA, AMD, and Intel GPUs on Fedora systems,
shows active drivers, installed packages, RPM Fusion repo status, and
provides one-click driver installation via a GTK4/libadwaita native UI.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --offline

%install
install -Dm755 target/release/fedora-gpu-manager \
    %{buildroot}%{_bindir}/fedora-gpu-manager

%files
%license LICENSE
%doc README.md
%{_bindir}/fedora-gpu-manager

%changelog
* Sat Apr 18 2026 Ray McCaffity <rxmccaf@gmail.com> - 1.0.1-1
- Initial COPR release with vendored dependencies
