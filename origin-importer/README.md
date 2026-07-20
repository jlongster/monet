# Origin Importer

This local adapter imports the private `OriginClient` from `/root/origin-client.js`; it does not copy
Origin credentials into the Monet repository.

Run it manually:

```sh
bun run origin-importer
```

It binds only to `127.0.0.1:47835`. Override the private client path or port with
`ORIGIN_CLIENT_PATH` and `ORIGIN_IMPORTER_PORT`.

New accounts fetch transactions from January 1 of the current year through today. Existing accounts
send a Monet-calculated `start_date` based on their latest posted and oldest pending transactions.
Override the new-account default with `ORIGIN_START_DATE=YYYY-MM-DD` and the end date with
`ORIGIN_END_DATE=YYYY-MM-DD`.

Configure Monet without starting a sync:

```sh
curl -X PUT http://127.0.0.1:47832/import \
  -H 'content-type: application/json' \
  --data '{"url":"http://127.0.0.1:47835"}'
```

Origin is fetched only after an explicit global sync from the UI (`s`) or API:

```sh
curl -X POST http://127.0.0.1:47832/import/run
```

The adapter refreshes Origin accounts at the start of a manual sync, then fetches each account's
transactions using Monet's requested date window. It never polls or schedules work.

Origin decimal-dollar transaction amounts are converted to Monet's signed integer cents at the
HTTP boundary.

Each account response includes one stable synthetic opening-balance transaction calculated as the
reported current balance minus all activity in the imported date window. Asset balances retain
their sign; Origin's positive amounts owed for liabilities are converted to negative ledger
balances. Repeated syncs update the same `origin-opening:{account_id}` source.

The current Origin client automatically refreshes its ID token through Cognito before expiry. The
adapter serializes account and transaction fetches so one sync performs at most one refresh. Replace
`/root/origin-client.js` only if Cognito can no longer refresh its credentials; the adapter detects
file modifications and reloads a newly generated client without a restart.
