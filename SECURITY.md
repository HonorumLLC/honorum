# Security policy

## Report a security defect

Report privately through GitHub: open the repository's **Security** tab and
choose **Report a vulnerability**. You can also email
[support@honorum.app](mailto:support@honorum.app) with the subject
`Honorum security report`.
Do not put exploit details or private data in a public issue.

Include the affected component, commit or version, reproduction steps,
and the expected effect. Use synthetic accounts and data when possible.
Never send passwords, Canvas tokens, session tokens, feed URLs, or private keys.

## Versions

Report defects in the current production release or the latest `beta` branch.
Include the commit hash for a source build.

## Local checks

Use disposable databases for tests. Keep provider credentials in environment
secrets or ignored local files. See [deployment](docs/deployment.md) for
credential storage and recovery procedures.
