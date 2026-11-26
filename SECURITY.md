# Security Policy

Eefi Browser is an experimental, early-development project. Please use the
latest stable release when evaluating or reporting a security issue. Older
releases are not guaranteed to receive security fixes. Beta and other
pre-release builds are provided for testing and are not recommended for
sensitive or production use.

## Reporting a Vulnerability

Please do not report security vulnerabilities in public issues, discussions,
pull requests, or reviews.

Use GitHub's
[private vulnerability reporting](https://github.com/kanezr/eefi-web-browser/security/advisories/new)
to send a report privately. If private reporting is unavailable, contact the
maintainer using contact details listed on their GitHub profile, if available.
Otherwise, open a minimal public issue asking for a private reporting channel;
do not include technical details until one is provided.

Include, when safe to do so:

- The affected Eefi Browser version, Windows version, and WebView2 Runtime
  version.
- A concise description of the security impact and affected component.
- Reproduction steps and a minimal proof of concept.
- Any relevant logs with personal information, credentials, tokens, and local
  file paths removed.

Do not attach browser profile databases, saved credentials, private browsing
data, or other sensitive user information.

## Safe Testing

Only test against software and systems you own or are explicitly authorized to
test. Avoid accessing, modifying, or deleting other people's data; service
disruption; or testing against third-party websites without permission. Use a
local test environment and the least intrusive reproduction that demonstrates
the issue.

## Scope

Reports are most useful when they demonstrate a security impact in Eefi
Browser, including its native Rust backend, browser UI/native command boundary,
profile and credential storage, WebView2 integration, or release and updater
flows. Vulnerabilities in upstream dependencies should also be reported to
their maintainers; please report them to this project as well if Eefi Browser
is affected.

Feature requests, non-security bugs, filter-list coverage gaps, and behavior
already documented as a limitation are not security reports unless they show a
separate security impact.

## Response and Disclosure

Maintainers will review reports on a best-effort basis. This project has no
guaranteed response or remediation timeline and does not currently offer a bug
bounty. Please allow time for investigation and a fix before making the
technical details public. Coordinated disclosure timing can be discussed with
the maintainers after a report is received.
