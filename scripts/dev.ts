import { mkdir } from "node:fs/promises"
import { createServer } from "node:net"
import { join } from "node:path"

const args = process.argv.slice(2)
const standalone = args.includes("--standalone")
const clientArgs = args.filter((argument) => argument !== "--standalone")

const availablePort = () => new Promise<number>((resolve, reject) => {
  const listener = createServer()
  listener.once("error", reject)
  listener.listen(0, "127.0.0.1", () => {
    const address = listener.address()
    if (!address || typeof address === "string") {
      listener.close()
      reject(new Error("Could not allocate a standalone Monet server port"))
      return
    }
    listener.close((error) => error ? reject(error) : resolve(address.port))
  })
})

const runClient = (env: Record<string, string | undefined>) => Bun.spawn(
  ["bun", "--watch", "src/index.tsx", ...clientArgs],
  {
    cwd: process.cwd(),
    env,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  },
)

if (!standalone) {
  const client = runClient(process.env)
  process.exit(await client.exited)
}

const directory = join(process.cwd(), ".money-tui", "standalone")
await mkdir(join(directory, "inference"), { recursive: true })
const port = await availablePort()
const serverUrl = `http://127.0.0.1:${port}`
const output = Bun.file(join(directory, "server.log"))
const server = Bun.spawn(
  ["cargo", "run", "--quiet", "--manifest-path", "server/Cargo.toml"],
  {
    cwd: process.cwd(),
    env: {
      ...process.env,
      MONET_SERVER_ADDR: `127.0.0.1:${port}`,
      MONET_DB_PATH: join(directory, "money.db"),
      MONET_LOG: join(directory, "monet.log"),
      MONET_INFERENCE_LOG_DIR: join(directory, "inference"),
    },
    stdin: "ignore",
    stdout: output,
    stderr: output,
  },
)

let ready = false
for (let attempt = 0; attempt < 300; attempt += 1) {
  if (server.exitCode !== null) break
  try {
    const response = await fetch(`${serverUrl}/health`)
    if (response.ok) {
      ready = true
      break
    }
  } catch {
    // The server is still starting.
  }
  await Bun.sleep(100)
}

if (!ready) {
  server.kill()
  await server.exited
  const log = await output.text().catch(() => "")
  throw new Error(`Standalone Monet server failed to start.\n${log}`)
}

const client = runClient({ ...process.env, MONET_SERVER_URL: serverUrl })
const stop = () => {
  client.kill()
  server.kill()
}
process.on("SIGINT", stop)
process.on("SIGTERM", stop)

const exitCode = await client.exited
server.kill()
await server.exited
process.exit(exitCode)
