# mise

`gnosis` is not in the built-in mise registry. Use the GitHub backend:

```bash
mise use -g github:SinAnima/gnosis@0.2.0
```

Or in `mise.toml`:

```toml
[tools]
"github:SinAnima/gnosis" = "0.2.0"
```

To keep a short name in `.tool-versions` (`gnosis 0.2.0`):

```toml
[tool_alias]
gnosis = "github:SinAnima/gnosis"
```

If multiple assets match, narrow with `matching = "gnosis-"`.
