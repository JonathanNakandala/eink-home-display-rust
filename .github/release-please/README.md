# How a release is made

Merging a release pull request (made by release-please from the conventional commits) does this, in `.github/workflows/release-please.yml`:

1. release-please writes the changelog and makes the release as a **draft**, because `"draft": true` is set in `config.json` for both packages. A draft
   is not public. GitHub would make its tag only when it is published, so `"force-tag-creation": true` has release-please make the tag at once, on the
   release commit: without it, the next run of release-please finds no release, takes the whole history as unreleased, and opens a second release pull request.
2. The checks run on that commit: `ci.yml` and `packages.yml` for the server (the program builds and tests, and the archive, the `.deb` and the `.rpm`
   install and start), `firmware.yml` for the firmware.
3. The server's job builds the release archive and the two packages and attaches them, with their checksums, to the draft.
4. **Last**, the job publishes the draft (`gh release edit TAG --draft=false`). This is when it becomes public and immutable (its tag is already there, made in step 1).

Why a draft: the repository's releases are **immutable** (a repository setting). Once a release is published its tag is locked to a commit and its files
cannot be added to, changed or removed, and GitHub signs a statement of what it holds (`gh release verify`). So everything has to be attached before it is published,
and nothing is public until everything has passed.

## When it goes wrong

- **A job fails before the last step:** nothing is public. The draft is there, under Releases (only people who can write to the repository see it). Fix what failed
  and use "Re-run failed jobs" on the workflow run: the same release-please outputs are used, so it is the same draft and the same version, `--clobber` replaces
  what the failed attempt attached, and the last step publishes it. If it cannot be fixed, delete the draft and its tag (`git push origin :refs/tags/TAG`), and a new commit makes the next version.
- **It was published and is wrong:** it cannot be edited. Delete the release (the tag name can then never be used again), and release the next version with a fix.
  Check the archive and packages before merging: `packages.yml` runs on every pull request that touches what they are made from, and does what the release does.
- **The setting is off:** nothing breaks; a published release is then an ordinary one that can be changed.

## The first release, and the changelog

There are no baseline tags, so the first release's notes list the whole history of the repository (see `esphome/README.md`, "The first release").
