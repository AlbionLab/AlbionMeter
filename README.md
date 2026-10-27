# Albion Meter

Damage and healing meter for Albion Online. It runs on your own computer, observes the network traffic the game client already receives and shows live DPS and HPS for your party. It never reads the game's memory, never changes the client and never sends anything to the game servers.

The interface is a local page served by the program at http://127.0.0.1:5187.

## Features

- Live damage and healing bars for the party, with DPS/HPS and share of each player.
- Play, Pause, Resume, Stop and Reset of a record.
- Copy the ranking as chat text or as an image.
- Capture through the system socket (no driver needed) or Npcap (Windows) / libpcap (Linux), on any network interface.
- Weapon, item and map names imported from your own game installation.
- Optional publishing of each record as a live session on [albionlab.app](https://albionlab.app), with the full analysis.

## Requirements

- .NET SDK 10 and Node.js 24 to build.
- Windows 10/11 (64-bit) or 64-bit Linux to run.

## Run from source

```bash
npm ci --prefix web
node scripts/dev.mjs
```

The script builds the interface and the server, then starts the meter. On Linux it asks once for the `cap_net_raw` capability after every build; on Windows the program asks for administrator rights.

Then:

1. **Game data** → **Import game folder** → choose the folder that contains `Albion-Online_Data`.
2. **Capture** → choose the method and interface → **Start capture**.
3. **Meter** → **Play**.

Records are stored in `encounters.db` inside the user's local data folder (`AlbionMeter`). Use `--Storage:Directory=<path>` to choose another folder.

## Configuration

| Setting | Default | Meaning |
| --- | --- | --- |
| `Storage:Directory` | user data folder | Where records and settings are stored. |
| `Capture:AutoStart` | `false` | Start the last capture interface when the program opens. |
| `Capture:Debug` | `false` | Store raw packets (bounded) in the database for troubleshooting. |
| `AlbionLab:Url` | `https://albionlab.app` | Site used to publish sessions. |

Settings can be passed as arguments (`--Capture:AutoStart=true`) or environment variables (`Capture__AutoStart=true`).

## Tests

```bash
dotnet run --project tests/AlbionMeter.Tests
npm test --prefix web
```

## Packages

```bash
node scripts/publish.mjs win-x64
node scripts/publish.mjs linux-x64
```

Each command publishes a self-contained build to `artifacts/<runtime>` and creates the matching `.zip` or `.tar.gz` package.

## Layout

- `src/AlbionMeter.Core` — packet capture, Photon decoding, combat observation and aggregation.
- `src/AlbionMeter.Server` — local server, record controls, history, AlbionLab publishing.
- `web` — the meter interface (React + TypeScript).
- `tests` — C# checks.
- `tools` — capture inspection and game data import command-line tools.
- `assets/game-data` — combat and map catalogs generated from the game data.
- `docs` — protocol and capture notes.
