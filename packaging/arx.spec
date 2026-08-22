%global debug_package %{nil}
# The Rust binary is already validated before packaging. Do not let rpmbuild
# strip or otherwise rewrite it; package smoke verifies byte-for-byte identity.
%global __strip /bin/true
# Keep the package payload exact. rpmbuild otherwise injects a /usr/lib/.build-id
# symlink that is outside ARX's executable/docs release contract.
%global _build_id_links none
# Release CI independently verifies the extracted RPM file/symlink manifest.

Name:           arx
Version:        %{arx_version}
Release:        1
Summary:        Terminal commander for local and remote workspaces
License:        MIT
URL:            https://github.com/mrAibo/arx
BuildArch:      x86_64

%description
ARX is a Linux terminal commander for local and remote workspaces, with
truthful background jobs, transfer controls, storage inspection, SFTP, S3,
and WebDAV support.

%prep

%build

%install
install -Dpm0755 %{arx_binary} %{buildroot}%{_bindir}/arx
install -Dpm0644 %{arx_readme} %{buildroot}%{_docdir}/arx/README.md
install -Dpm0644 %{arx_license} %{buildroot}%{_licensedir}/arx/LICENSE
install -Dpm0644 %{arx_third_party} %{buildroot}%{_docdir}/arx/THIRD_PARTY_LICENSES.html

%files
%{_bindir}/arx
%doc %{_docdir}/arx/README.md
%license %{_licensedir}/arx/LICENSE
%doc %{_docdir}/arx/THIRD_PARTY_LICENSES.html

%changelog
* Thu Jan 01 1970 ARX maintainers <noreply@example.invalid> - %{arx_version}-1
- Automated upstream release package
