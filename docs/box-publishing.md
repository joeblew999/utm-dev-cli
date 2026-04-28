# Pre-baked VM box publishing

After bootstrapping a VM (VS Build Tools, mise, Rust toolchain, WebView2, multiarch deps, etc.), you can package its current state as a Vagrant `.box` file and publish it. New devs then download a fully-bootstrapped image and skip the 30+ min setup.

## Workflow

### 1. Package locally

```sh
utm-dev vm package --name windows-build
# → produces .build/boxes/windows-11-windows-build_arm64.box (~6 GB)
```

The packaged box wraps the `.utm` bundle directly out of UTM's storage. It does NOT include any project source — that gets synced fresh by `vm build` on each dev's machine.

### 2. Upload to Cloudflare R2

```sh
# Bucket setup once: dashboard or wrangler
wrangler r2 bucket create utm-dev-boxes

# Upload (use a versioned key — bumping the version busts the cache on
# downstream devs):
wrangler r2 object put \
  utm-dev-boxes/windows-build/v1.box \
  --file=.build/boxes/windows-11-windows-build_arm64.box \
  --remote
```

### 3. Make the bucket public

Either:
- **Custom domain (recommended):** `r2.joeblew999.com` mapped via Cloudflare dashboard. URL: `https://r2.joeblew999.com/utm-dev-boxes/windows-build/v1.box`.
- **Public R2 dev URL:** dashboard → R2 → bucket → "Public access" toggle. URL: `https://pub-<account-id>.r2.dev/<bucket>/<key>`.

### 4. Wire the URL into utm-dev

Edit `src/vm/profiles.rs` and set `prebaked_url` on the relevant profile:

```rust
VmProfile {
    name:       "windows-build",
    // ...
    prebaked_url: Some("https://r2.joeblew999.com/utm-dev-boxes/windows-build/v1.box"),
},
```

Tag a release. New devs running `utm-dev vm up --name windows-build` now download the pre-baked box instead of building from scratch.

## How `vm up` consumes a pre-baked box

`import::ensure_imported` checks `profile.prebaked_url` first:

- **Set** → `download_prebaked` fetches the URL directly (with HTTP `Range:` for resume support), caches at `~/.cache/utm-dev/<box>_prebaked_arm64.box`, then imports into UTM via the same AppleScript path as a Vagrant Cloud box.
- **None** → falls back to current Vagrant Cloud lookup.

Bootstrap still runs after import, but every step short-circuits because all markers are present (mise installed, VS Build Tools at expected path, WebView2 dir exists, rustup default-host = x86_64). Bootstrap completes in ~10 sec instead of 25+ min.

## Cost estimate (Cloudflare R2)

- Storage: ~6 GB × $0.015/GB-month = **$0.09/month per published box**
- Egress: free (R2's whole point)
- Class A operations (puts): negligible at this volume

## Versioning

Use a versioned key in the URL: `windows-build/v1.box`, `v2.box`, etc. Bumping the path forces fresh downloads. The local cache key uses the URL-derived name, so a new URL = new cache slot, no stale data.

## What about Microsoft's EULA?

Vagrant's `utm/windows-11` registry box uses a legitimately-distributable Windows variant (eval edition). Re-packaging it with VS Build Tools + your dev tooling on top is the same delivery model — same as repackaging Ubuntu with extra apt packages. If in doubt, talk to a lawyer; this doc is not legal advice.
