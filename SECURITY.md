# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private
vulnerability reporting on this repository (the **Security** tab → **Report a
vulnerability**). If that is not available to you, contact the maintainer
privately on GitHub: [@jaylikesbunda](https://github.com/jaylikesbunda).

Include what you can: the version, what happens, how to reproduce it, and what
you think the impact is. You will get an acknowledgement as soon as possible,
and credit in the release notes unless you would rather stay anonymous.

## Supported versions

Fixes are made on the latest release. Older releases are not maintained; please
test against the newest tag before reporting.

## Scope

Rhumb is a local desktop program. It opens files, archives and folders, parses
Markdown, and fetches remote images that a Markdown document points at. The
parts most worth a careful look:

- Archive handling - path traversal when extracting (`zip`, `tar`, `7z`, `rar`).
- The Markdown preview - remote image fetching, SVG rasterisation, and the
  layout that draws untrusted documents.
- Anywhere untrusted file names, sizes or contents are trusted.

Anything that lets a crafted file, archive or document read or write outside
where it should, run code, or crash the program without a clear message is in
scope. The design notes in the module headers say which invariants are load
bearing; a report that breaks one is especially welcome.
