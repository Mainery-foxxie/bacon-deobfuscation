# bgdeobf: Bacon Guard / FORGE deobfuscator bot

Turns Roblox scripts protected with **Bacon Guard / FORGE** (a Luau VM that uses `buffer`/`bit32`) back into readable
Luau. There are two ways in: the **CLI** and a **Discord bot**. The engine only needs Python 3.10+ and the Luau tools
(bundled for Linux x86-64).

Only analyze scripts you own or are allowed to analyze.

## How it works (short)
1. The loader is read through its AST (`luau-ast`), and the VM pieces are recognized from its structure.
2. The patched loader runs in a plain Luau sandbox (no `io`, with time/memory/output limits) only to dump the VM
   contents. The program inside the VM is **not** run.
3. Every handler of that build is recognized by running its code in a small interpreter and comparing it with the
   Luau instruction spec. There is no hand-written per-build table.
4. The instructions are decompiled to Luau, and variable names are guessed automatically (or taken from a names file).
5. The result is checked: the syntax must pass `luau-compile`, then the bytecode of the result is compared with the
   program that was unpacked.

## Install & try
```
pip install -r requirements.txt          # only needed for the Discord bot
python -m bgdeobf --check                # check the Luau tools
python -m bgdeobf script.lua             # -> script_deobf.lua
python -m bgdeobf script.lua -o result.lua --annotate     # write every variable's key (-> a names file)
python -m bgdeobf script.lua --names names.json           # use your own names: {"12:34": "playerList"}
python -m bgdeobf --detect script.lua    # only say if it is Bacon Guard / FORGE and which build (nothing is run)
python -m bgdeobf scripts/ -o results/ --jobs 4           # a whole folder (or several files) at once
```
Exit codes: 0 success, 1 misuse, 2 failure (the reason is printed clearly). With several files, 2 means at least one
failed; the others are still written.

### Several files, a folder and `--detect`
- **A folder** means the `.lua`, `.luau` and `.txt` files directly inside it (not its sub-folders). Results of an earlier
  run (`*_deobf.lua`, `*_annotated.lua`) found in a folder are skipped, so running the same folder twice is safe.
  Results go next to each input, or into the folder given with `-o` (created if needed; two inputs with the same name
  become `x_deobf.lua` and `x_2_deobf.lua`). With one file, `-o` is still the output file (or a folder, if it is one).
- **`--jobs N`** processes N files at the same time, each in its own limited worker process. One broken file never
  stops the others. `--json` prints one entry per file.
- **`--detect`** parses only the loader (nothing is run, not even in the sandbox) and prints one line per file:
  `name`, `bacon_guard` or `unknown`, a confidence and the build. `1.00` means the loader layout is recognized, `0.50`
  means the banner is there but the layout is not (damaged, truncated, or a newer build), `0.00` means it is not a
  Bacon Guard / FORGE script. A `1.00` does not promise a full result: the handlers of the build are only checked
  when the script is really deobfuscated. The exit code is 2 if any file is not recognized.

### Naming variables (`annotate` + names file)
`--annotate` (the `annotate` option of `/deobf`) writes a key after every variable, for example
`local Players<@12:34> = game:GetService("Players")`. That copy is **not runnable Luau**: it is only there so you can
read the keys and choose names. Put your names in a JSON file such as `{"12:34": "playerList"}` and run again with
`--names` (the `names` option of `/deobf`) to get clean code. The annotated copy is saved as `<name>_annotated.lua`
(the clean one as `<name>_deobf.lua`), and the syntax/bytecode checks always run on a clean copy of the same code.

## Discord bot
1. discord.com/developers/applications → New Application → Bot → Reset Token → copy the token.
2. Still on the Bot page, turn on **MESSAGE CONTENT INTENT** (Privileged Gateway Intents). It is only needed for
   `!uwu`; without it set `PREFIX_COMMANDS=0` in `.env` and use slash commands only.
3. `cp .env.example .env` and fill in `DISCORD_TOKEN=`. (Never share `.env`.)
4. `python -m bgdeobf.bot`
5. Invite the bot: OAuth2 → URL Generator → scopes `bot` + `applications.commands`. Permissions `!uwu` uses:
   View Channel, Send Messages, Attach Files, Read Message History. Slash commands need no channel permissions.

How to use it:

| Command | What it does |
|---|---|
| `!uwu` + attachment | deobfuscates the attached `.lua` / `.luau` / `.txt` file; the result is a reply in the channel |
| `!uwu <raw url>` | downloads from the URL, then deobfuscates. You can also paste the whole line: `!uwu loadstring(game:HttpGet("https://…"))()` |
| reply to a message with `!uwu` | uses the file or URL in the message you replied to |
| `/deobf file:…` or `/deobf url:…` | same, but only you can see the result (`private`, on by default); options `annotate`, `names` |
| right-click a message → Apps → **Deobfuscate file** | uses the attachment or URL in that message (private result) |
| `/deobf-info`, `!uwu help` | help |

GitHub `blob` links, normal Pastebin links and Gists are converted to their raw form automatically. Links to web pages
(HTML) are rejected with a hint.
`!uwu` replies in the channel, so everyone there can see the result; use a slash command if you want it private.

Settings in `.env` (all optional except the token):

| Name | Default | Meaning |
|---|---|---|
| `GUILD_ID` | empty | register the slash commands to one server only (they appear immediately, good for testing) |
| `ALLOWED_GUILDS`, `ALLOWED_USERS` | empty = everyone | restrict who can use the bot (IDs separated by commas); applies to slash commands and `!uwu` |
| `PREFIX`, `PREFIX_COMMANDS` | `!`, 1 | prefix of the chat command; `0` turns `!uwu` off (and then the Message Content Intent is not needed) |
| `URL_HOSTS` | empty = any public host | limit URLs to certain domains, e.g. `raw.githubusercontent.com, pastebin.com` |
| `MAX_MB` | 3 | maximum file size (attachments and URLs) |
| `WORKERS`, `MAX_QUEUE` | 2, 8 | parallel jobs and queue length |
| `JOB_TIMEOUT` | 240 | hard time limit per job (seconds); the process is killed when it runs over |
| `COOLDOWN` | 15 | pause per user (seconds) |

One user can only have one job running at a time.

## Limits and safety
- Every job runs in a separate process with a memory limit and a hard time limit; the Luau sandbox has CPU, memory and
  output limits and no `io`. When a job finishes or is cancelled, all its processes and its temporary folder are cleaned up.
- URL downloads are restricted so nobody can use the bot to reach your server's network: only `http`/`https` on
  port 80/443, no `user:password`, **every** IP address returned by DNS must be public (loopback, private networks,
  link-local/cloud metadata, local IPv6 and IPv4 addresses wrapped in IPv6 are rejected; the check lives in the resolver
  the connection uses, so there is no DNS-rebinding gap), every redirect is checked again (max 3), size limits (also
  after decompression) and time limits apply, no cookies are sent, and proxies from the environment are not used.
- Files are processed in memory and a temporary folder; the log records only the user id, the size and the outcome, never
  the contents. Messages shown in Discord are scrubbed of file paths from the machine running the bot.
- For a bot that is open to the public, run it in a separate VM/container and use `ALLOWED_GUILDS`.
- If the script is not Bacon Guard, is truncated, is too large, or the build uses handler patterns that are not
  recognized yet, the result is a **clear error**, not a guess.

## Luau binaries
`luau_bin/` holds the official Luau binaries (`luau`, `luau-ast`, `luau-compile`; MIT license, see
`luau_bin/LICENSE-luau.txt`). The bundled ones are for Linux x86-64 (glibc 2.34+, e.g. Ubuntu 22.04+). On another OS,
download `luau`, `luau-ast` and `luau-compile` from the official luau-lang/luau releases, put them in one folder and
set `BGDEOBF_LUAU_DIR=<that folder>`. On Windows/macOS the sandbox memory limit is not active (the time limit still is).
`python -m bgdeobf --check` shows whether the tools can run.

## What has and hasn't been tested
- Tested on 10 real builds (Bacon Guard v0.42.0 "The Morgue Shift", ClarksCicaverse, TommyPlay, FORGE Protection build
  03d3bec6a6b0, a build with no banner that keeps the program in compressed pages, and Bacon Guard v0.43.0 | FORGE
  Protection builds baf6ea30ebd0, cf776ebed029, a7c98b93da5b, af206410f1dd and cb5687cb3df4). Bytecode check, in that
  order: 168/168, 24/24, 172/172, 93/93, 172/172, 70/70, 62/62, 77/77, 58/58 and 85/85 functions identical, meaning the
  result compiles back to the same instructions in the same order (registers are not compared, since they can differ
  for the same source). On another build the check can still report a function as "reordered" (same instructions, a
  plain read or calculation in a different place) or "cosmetic" (one `return` more or less); the summary says so.
  `--detect` and a whole-folder run with `--jobs 3` were also tried on those 10 scripts: all 10 were recognized and all
  10 results came out identical.
  Other builds may use patterns that are not covered yet; read the error message. The protector changes small details
  between builds (for example how the loader ends), so a new build can fail with a clear error message until that
  detail is added.
- v0.43.0 changed how the VM stores a program: every function's registers are shifted by a per-function number, a loop
  can be one record that holds the whole loop, and a closure record carries its own capture list. All three are handled.
  Not handled: a closure whose captures are separate records in that newer layout (the sample has none), and `...`
  assigned into variables that were declared earlier (raises an error instead of guessing).
- Variable names are only guesses; the result is checked for shape, not for names. A name is never the same as a
  Roblox or executor function, so a variable can not hide a function the script calls (`local readfile = readfile(x)`
  is written `local readfile2 = readfile(x)`). `EXECUTOR_GLOBALS` in `gen.py` holds every global of the Real executor
  from https://docs.projectreal.gg/en (232 functions, their aliases and library tables such as `crypt` and `json`);
  every global the script itself reads is kept free as well, whoever defines it.
- Tables follow the bytecode: a constructor the compiler turned into a template (`{a = 1, b = f()}`) is printed as one,
  and a table it built field by field is printed as `local t = {}` plus assignments. That is the same program but can
  look less tidy than the original source (for example two tables that were arguments of one call become two locals).
  The same goes for a value that the compiler computed into a variable before it was used (`local size = p.Size` and
  then `size.Y * 0.5`): it is kept as a local, because writing it into the expression would move its read.
- The result has a blank line around every block (function, `if`, loop) and between two short
  `if ... then return end` guards, so long scripts read more easily. It changes nothing in the program.
- About 280 automated tests (`python -m pytest -q tests/`; the ones that need a protected script are skipped when
  `tests/samples/` is empty): sandbox limits, damaged/truncated files, hard timeout, queue, the URL downloader (local
  server + fake resolver), the command line (single file, folders, `--jobs`, `--detect`), the decompiler and the decoder
  on small hand-assembled programs (these need no protected script), and the whole bot flow (slash, menu, `!uwu`)
  using the **real discord.py library with a fake Discord network**.
  A live connection to Discord could **not** be tested where this was built (no token). To check on
  your machine: set `GUILD_ID`, start the bot, send one file with `/deobf` and one with `!uwu`. Downloading from the real
  internet (e.g. raw.githubusercontent.com) could not be tried there either; the TLS path uses aiohttp's defaults.
- Windows: a real run with Windows Luau binaries worked on an earlier version of this tool (this version was not run
  there). macOS is untested.
- Tests that need a protected script are skipped when `tests/samples/` is empty (the scripts are not included in the
  zip). Put a protected script there to run them.

## Want Telegram or a web page?
All the logic is in `bgdeobf.runner.run(data, Options())` (it returns `Result.text` and `Result.info`) and the queue
`bgdeobf.service.Service`; `bot.py` is only a thin layer on top of them.

## Contents
| File | Purpose |
|---|---|
| `engine.py` | the whole pipeline + checking the result |
| `runner.py`, `worker.py` | run the engine in a separate process with a hard time limit |
| `service.py` | queue, one job per user, progress |
| `bot.py` | Discord bot (slash, menu, `!uwu`) |
| `fetch.py` | safe URL downloader (anti-SSRF) |
| `loader.py`, `patch.py`, `dumper.lua.tpl`, `sandbox.py` | recognize the loader, unpack the VM |
| `classify.py`, `luavm.py`, `refspec.py` | recognize the handlers of each build |
| `decode.py`, `decomp.py`, `gen.py`, `post.py`, `printer.py`, `render.py`, `fidelity.py` | decompiler and verification |
| `messages.py` | the texts shown to users (and the path scrubbing) |
