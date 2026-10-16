# mise

`gnosis` is not in the built-in mise registry. Use the GitHub backend, which reads [GitHub Releases](https://github.com/SinAnima/gnosis/releases):

```bash
mise use -g github:SinAnima/gnosis@latest
```

Or in `mise.toml`:

```toml
[tools]
"github:SinAnima/gnosis" = "latest"
```

To keep a short name in `.tool-versions` (`gnosis latest`):

```toml
[tool_alias]
gnosis = "github:SinAnima/gnosis"
```

If multiple assets match, narrow with `matching = "gnosis-"`.
