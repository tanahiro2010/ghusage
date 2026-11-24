# Dependency audit exceptions

The documentation site is a static build. Its npm dependencies are used only
while building or previewing the site and are not shipped to users.

Until VitePress 2 is stable, the current VitePress 1 toolchain retains the
following development-only advisories reported by `npm audit`:

- `vite` high severity advisory
- `esbuild` moderate severity advisory
- `vitepress` moderate severity advisory

The documentation CI gates production dependencies with `npm audit` and
`--omit=dev`, and also runs a non-blocking full audit so new development-only
findings remain visible in the workflow log. These exceptions must be
revisited when the VitePress 2 release is stable.
