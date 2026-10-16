# Homebrew formula

Source of truth for the formula. `scripts/publish-install-channels.sh` writes
`gnosis.rb` from the release `SHA256SUMS` file and publishes it to
[`SinAnima/homebrew-tap`](https://github.com/SinAnima/homebrew-tap) as `Formula/gnosis.rb`.

The release workflow runs that script with the `INSTALL_REPOS_TOKEN` secret. The token needs contents write on `homebrew-tap`, `scoop-bucket`, and `asdf-gnosis`.

```bash
brew install SinAnima/tap/gnosis
```
