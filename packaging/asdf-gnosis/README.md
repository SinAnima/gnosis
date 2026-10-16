# asdf-gnosis

asdf plugin that installs `gnosis` from [GitHub Releases](https://github.com/SinAnima/gnosis/releases).

```bash
asdf plugin add gnosis https://github.com/SinAnima/asdf-gnosis.git
asdf install gnosis latest
asdf global gnosis latest
gnosis --version
```

`list-all` reads the release tags. A release that has no matching archive for the current OS and architecture fails at install time.
