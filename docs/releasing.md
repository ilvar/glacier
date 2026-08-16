# Releasing Glacier

Glacier follows Semantic Versioning. `package.version` in `Cargo.toml` is the
single source of truth, and the compiled binary exposes it with:

```sh
legacy --version
```

## Version on every commit

Every commit must change the application version. The normal case is an automatic
patch bump (`0.1.0` -> `0.1.1`). `commit.sh` performs this before committing, and
the pre-commit hook enforces the same rule for direct `git commit` usage.

For a feature or breaking change, bump deliberately before committing:

```sh
./scripts/bump-version.sh minor
./scripts/bump-version.sh major
```

The automatic hook compares the working version with `HEAD`; if it has already
changed, it does not add another patch bump.

## Automatic releases from main

Every push to `main` is a release. In normal development this means every merged
pull request gets the version already recorded in its `Cargo.toml`.

The release workflow first checks that `vX.Y.Z` does not already exist. It then:

1. builds and publishes `linux/amd64` and `linux/arm64` images to
   `ghcr.io/ilvar/glacier`;
2. tags the image as `latest`, `X.Y.Z`, `X.Y`, and `sha-*`;
3. packages and publishes the Helm chart with chart/app version `X.Y.Z` to
   `oci://ghcr.io/ilvar/charts/glacier`;
4. creates git tag `vX.Y.Z` at the main commit and a GitHub Release with generated
   release notes.

There is no separate manual release commit or tag step. If a main push reuses an
existing version, the workflow fails before publishing.

Pull requests run the Docker build plus Helm lint/render checks without publishing
packages or creating a release.

## Version model

Release chart packages use their `appVersion` as the default container image tag.
The source checkout uses `latest`; either can be overridden with
`image.repository`, `image.tag`, and `image.pullPolicy`.

For this single-crate application, Cargo SemVer remains preferable to deriving the
product version from `git describe`: builds from source archives stay reproducible
and carry the same version as builds made from a git checkout.
