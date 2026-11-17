# Changelog

The Map's own history. Its version is the one in `manifest.json`, separate from PM's (spec D12): a
change that touches only this folder bumps this version and adds an entry here.

## 1.0.0

- Skeleton only: the folder, its manifest and a stub page. The Map itself is built in the slices that
  follow, and `publisher.commit` stays all zeros in the source tree (a manifest cannot hold the hash of
  the commit that contains it). `pack-mod --commit <sha>` stamps the real one into the package.
