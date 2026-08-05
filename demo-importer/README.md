# Demo Importer

The demo is an example transaction import plugin. Configure it in `~/.config/monet/config.json`:

```json
{
  "import": {
    "command": ["bun", "demo-importer/server.ts"],
    "cwd": "/absolute/path/to/monet",
    "env": [],
    "config": {},
    "initializeTimeoutMs": 5000,
    "requestTimeoutMs": 60000
  }
}
```

Run the import:

```sh
curl -X POST http://127.0.0.1:47832/import/run
```

Inspect imported accounts:

```sh
curl http://127.0.0.1:47832/accounts
```

The demo includes checking and savings accounts plus income and expense account hierarchies. Every
request for Checking transactions generates 3-5 new fake purchases, so every global Monet sync adds
visible data. IDs remain stable for the lifetime of the plugin process.
