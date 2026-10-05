# Distribution: channels, accounts and keys

What each distribution channel needs: accounts, keys and tokens, who holds them, and what happens after. The build side is automated (`cargo xtask dist`, see [packaging/README.md](../packaging/README.md)); the steps below need the maintainer's own accounts.

**Never paste a token or private key into a chat, an issue or a commit.** Tokens live in files only the owner can read (`chmod 600`) or in GitHub repository secrets.

## Summary

| Channel | Status | Account | Keys / tokens | Cost | Review |
|---|---|---|---|---|---|
| GitHub Releases | **Done** (v0.1.0) | GitHub | `gh` CLI login (already set up) | free | none |
| COPR (Fedora RPM repo) | Next | Fedora Account (FAS) | COPR API token | free | none (automatic builds) |
| extensions.gnome.org | Next | EGO account | none | free | human review, days to weeks |
| Flathub (the app only) | Later | GitHub | none (Flathub signs) | free | human review of the manifest |
| Fedora official repositories | Optional, long-term | FAS + packager group | Kerberos, SSH key, FAS 2FA | free | package review |
| Release signing | Optional | — | GPG key (or Sigstore) | free | — |

## 1. GitHub Releases (done)

- Repository: https://github.com/KhaledSaeed18/drainscope (public).
- Release v0.1.0 holds the source tarball, the SRPM, the binary RPMs and the extension zip. The spec's `Source0` points at the tarball there, so COPR can fetch it.
- To release again: bump the versions (packaging/README.md, "Releasing"), run `cargo xtask dist --rpm`, then `git tag -a vX.Y.Z` and `gh release create`.
- The RPMs on GitHub are **unsigned**. `dnf install ./file.rpm` accepts that for local files, but users should prefer COPR, whose packages are signed (below).

## 2. COPR: Fedora package repository

Users get `sudo dnf copr enable khaledsaeed18/drainscope` and updates through `dnf upgrade`.

### Accounts and keys

1. **Fedora Account (FAS):** sign up at https://accounts.fedoraproject.org. Use a real email; enabling two-factor authentication is recommended.
2. **Log in to COPR once** at https://copr.fedorainfracloud.org (top right, "log in" with the Fedora account). This creates your COPR user (`khaledsaeed18`, or whatever your FAS username is).
3. **API token:** open https://copr.fedorainfracloud.org/api while logged in. It shows a config block like:
   ```ini
   [copr-cli]
   login = …
   username = khaledsaeed18
   token = …
   copr_url = https://copr.fedorainfracloud.org
   ```
   Save it as `~/.config/copr` with `chmod 600 ~/.config/copr`. The token **expires after 180 days**. COPR emails before it does; renew it on the same page.
4. **Signing key:** none needed. COPR creates a GPG key for each project and signs every RPM with it; `dnf copr enable` installs that key for users.

### After the token is in place

These are the commands I'll run once the token is in place. You can also run them yourself:
```bash
sudo dnf install copr-cli
copr-cli create drainscope \
  --chroot fedora-44-x86_64 --chroot fedora-rawhide-x86_64 \
  --description "Per-app battery and energy usage for the Linux desktop" \
  --instructions "sudo dnf copr enable khaledsaeed18/drainscope && sudo dnf install drainscope drainscope-sampler"
copr-cli build drainscope target/dist/rpmbuild/SRPMS/drainscope-0.1.0-1.fc44.src.rpm
```
- **Build requirements:** clang, libbpf-devel, kernel-headers, selinux-policy-devel, appstream and desktop-file-utils all come from Fedora, and the vendored crates mean no network is needed during the build.
- **aarch64:** add `--chroot fedora-44-aarch64` once someone can test on ARM. The eBPF build is architecture-aware, but RAPL is x86-only, so ARM machines only get battery-only mode.
- **Automatic builds (optional):** in the COPR project, open Settings → Integrations, copy the GitHub webhook URL, and add it to the GitHub repository under Settings → Webhooks. New tags then build automatically. No token goes into GitHub for this.

## 3. extensions.gnome.org (EGO)

### Account
- Register at https://extensions.gnome.org/accounts/register/. This is separate from GitLab and the Fedora account. No keys are involved.

### Before uploading
- **Zip:** use `target/dist/drainscope@khaledsaeed18.github.io.shell-extension.zip`, or the one attached to the GitHub release.
- **Screenshot:** take one of the Battery tile open in Quick Settings. EGO shows it on the listing.
- **Description:** must say the extension needs the drainscope daemon, installed from COPR or from source; without it, the tile just reads "Daemon not running". Link the README's install section.

### Review
- The review checks: GPL-compatible license (ours is GPL-3.0-or-later), unminified code, nothing created before `enable()`, everything cleaned up in `disable()`, and no synchronous I/O in the Shell process. The extension was written to these rules.
- Expect days to a few weeks for the first review. Each new version (including each new GNOME release, when `shell-version` gains "51") is reviewed again.
- Reviewers may ask questions in the upload's review thread. Their comments arrive by email.

## 4. Flathub (the desktop app; later)

- **Scope:** only `drainscope-app` fits a Flatpak; the daemon, sampler and probe are system services and stay RPMs. The Flatpak talks to the daemon with `--talk-name=io.github.khaledsaeed18.Drainscope.Monitor`.
- **Account:** a GitHub account is enough. App IDs under `io.github.<user>` are verified by logging in to Flathub with that GitHub account.
- **Keys:** none. Flathub builds and signs everything.
- **Needed first:**
  - screenshots in the AppStream metainfo (required);
  - a Flatpak manifest on the `org.gnome.Platform` 50 runtime;
  - a pull request to https://github.com/flathub/flathub, which is reviewed.

## 5. Fedora official repositories (optional, long-term)

Getting into Fedora itself (`sudo dnf install drainscope` with no COPR) needs:

- **Accounts and access:**
  - a FAS account in the **packager** group, which needs a sponsor who reviews your first package;
  - an SSH key uploaded to FAS, a Kerberos login (`fkinit`) and 2FA.
- **Review:** a package review request in Red Hat Bugzilla, against Fedora's Rust packaging guidelines.
- **The big cost:** those guidelines **forbid vendored crates**. Every dependency not already in Fedora (e.g. `aya`, `aya-obj`, `zbus` versions) must be packaged and reviewed separately first.
- **SELinux:** the policy modules must follow Fedora's policy-module guidelines (`selinux-policy` macros, which the spec already uses).

COPR covers Fedora users well in the meantime.

## 6. Signing releases (optional)

- **Git tags:** to sign them, create a GPG key, add the public key to GitHub (Settings → SSH and GPG keys) and to https://keys.openpgp.org, then tag with `git tag -s` instead of `-a`. Tags and releases then show as "Verified".
- **Tarballs:** sign them with `gpg --armor --detach-sign drainscope-X.Y.Z.tar.gz` and attach the `.asc`. The spec can then verify them (`Source1: …asc`, `%gpgverify`).
- **Keyless alternative:** Sigstore (`cosign sign-blob`) needs no long-lived key, only a GitHub login at signing time.

## What I need from you, in order

1. **COPR:** create the Fedora account, log in to COPR once, and save the API token as `~/.config/copr` (`chmod 600`). Then tell me, and I'll create the project, submit the build and add the install instructions to the README.
2. **EGO:** register, take a screenshot of the tile, upload the zip with the description above, and answer the reviewers.
3. **Optional:** a GPG key for signed tags and tarballs, if you want "Verified" releases.
