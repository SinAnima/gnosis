# Scoop manifest

`scripts/publish-install-channels.sh` writes `gnosis.json` from the release `SHA256SUMS` file and publishes it to [`SinAnima/scoop-bucket`](https://github.com/SinAnima/scoop-bucket).

```powershell
scoop bucket add SinAnima https://github.com/SinAnima/scoop-bucket
scoop install gnosis
```
