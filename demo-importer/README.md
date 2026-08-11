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
    "requestTimeoutMs": 120000
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

The demo includes Personal, Household, and Travel accounts under Checking, a savings account, and
income and expense account hierarchies. Every request for a checking account's transactions
generates 3-5 new fake purchases, so every global Monet sync adds visible data to each checking
account. IDs remain stable for the lifetime of the plugin process.
