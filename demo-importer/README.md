# Demo Importer

Run the fake importer from the Monet repository root:

```sh
bun run demo-importer
```

It listens on `http://127.0.0.1:47833` by default. Override the port with
`DEMO_IMPORTER_PORT`.

Configure the running Monet server:

```sh
curl -X PUT http://127.0.0.1:47832/import \
  -H 'content-type: application/json' \
  --data '{"url":"http://127.0.0.1:47833"}'
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
request for Checking transactions generates 3-5 new fake purchases, so every global Monet sync
adds visible data. IDs remain stable for the lifetime of the demo process, and re-running the import
demonstrates Monet's saved `after` cursor.
