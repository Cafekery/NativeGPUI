# Security

## Reporting a problem

Report security problems privately through GitHub: on [Cafekery/NativeGPUI](https://github.com/Cafekery/NativeGPUI), open the Security tab and click Report a vulnerability.

Please don't open a public issue for anything that could lose or expose files. Found'er copies, moves, trashes and deletes files. It indexes the names of every file on the startup disk into `~/Library/Application Support/Founder`. It can change the system's `NSFileViewer` default. A bug in any of these can cost someone their data, so give us a chance to fix it before it's public.

Include the steps to reproduce it, the macOS version, and the commit you built from.

## Supported versions

Only the latest commit on `main` gets fixes.

## Response

This is a small project kept up in spare time. We answer and fix reports on a best-effort basis, with no promised response time.
