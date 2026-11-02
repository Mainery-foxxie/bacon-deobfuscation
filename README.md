# bgdeobf: Bacon Guard / FORGE deobfuscator bot

Turns Roblox scripts protected with **Bacon Guard / FORGE** (a Luau VM that uses `buffer`/`bit32`) back into readable
Luau. There are two ways in: the **CLI** and a **Discord bot**. The engine only needs Python 3.10+ and the Luau tools
(bundled for Linux x86-64).

It can also run a script in a **fake Roblox** (no network, no files, simulated time) and write down what it does: the
addresses it asks for, the remotes it fires, the windows it builds, the code it passes to `loadstring`, kicks,
teleports. That works on the deobfuscated result (`--trace`) or on a script as it is (`--sandbox`); see
[Fake Roblox sandbox](#fake-roblox-sandbox---trace-and---sandbox).

Scripts protected with **MoonVeil** (another protector, found by its banner) cannot be turned back into source by any tool.
For those you get a **behaviour reconstruction** instead: the script is run once against recording fake Roblox objects and
what it did is written down as a Lua-like file that says, in its first lines, that it is not the original. In the bot that
is `!moonveil`; see [MoonVeil](#moonveil-behaviour-reconstruction-not-source).

Only analyze scripts you own or are allowed to analyze.

## How it works (short)
1. The loader is read through its AST (`luau-ast`), and the VM pieces are recognized from its structure.
2. The patched loader runs in a plain Luau sandbox (no `io`, with time/memory/output limits) only to dump the VM
   contents. The program inside the VM is **not** run. (Only the optional `--trace` / `--sandbox` run a script, and
   then only in the fake Roblox described below.)
3. Every handler of that build is recognized by running its code in a small interpreter and comparing it with the
   Luau instruction spec. There is no hand-written per-build table.
4. The instructions are decompiled to Luau, and variable names are guessed automatically (or taken from a names file).
5. The result is checked: the syntax must pass `luau-compile`, then the bytecode of the result is compared with the
   program that was unpacked.
6. Optionally, the result (or the script as given) runs in a fake Roblox that logs what it does (`--trace`, `--sandbox`).

A MoonVeil script skips steps 1 to 5 (there is no source to recover): it is run in a recording harness and the calls it
made are written down.

## Install & try
```
pip install -r requirements.txt          # only needed for the Discord bot
python -m bgdeobf --check                # check the Luau tools
python -m bgdeobf script.lua             # -> script_deobf.lua
python -m bgdeobf script.lua -o result.lua --annotate     # write every variable's key (-> a names file)
python -m bgdeobf script.lua --names names.json           # use your own names: {"12:34": "playerList"}
python -m bgdeobf --detect script.lua    # only say if it is Bacon Guard / FORGE and which build (nothing is run)
python -m bgdeobf scripts/ -o results/ --jobs 4           # a whole folder (or several files) at once
python -m bgdeobf script.lua --trace     # deobfuscate, then run the result in the fake Roblox -> script_trace.txt
python -m bgdeobf script.lua --sandbox   # do not deobfuscate: run the file as it is in the fake Roblox
python -m bgdeobf hub.lua                # a MoonVeil script (found by its banner) -> hub_reconstructed.lua, not source
python -m bgdeobf hub.lua --trace        # ... and hub_trace.txt: the fake Roblox report of the script as given
```
Exit codes: 0 success, 1 misuse, 2 failure (the reason is printed clearly). With several files, 2 means at least one
failed; the others are still written.

### Several files, a folder and `--detect`
- **A folder** means the `.lua`, `.luau` and `.txt` files directly inside it (not its sub-folders). What an earlier run
  wrote (`*_deobf.lua`, `*_annotated.lua`, `*_reconstructed.lua`, `*_trace.txt`, `*_payload<N>.lua` / `.txt`) found in a
  folder is skipped, so running the same folder twice is safe; a file named on the command line is always taken.
  Results go next to each input, or into the folder given with `-o` (created if needed; two inputs with the same name
  become `x_deobf.lua` and `x_2_deobf.lua`). With one file, `-o` is still the output file (or a folder, if it is one).
- **`--jobs N`** processes N files at the same time, each in its own limited worker process. One broken file never
  stops the others. `--json` prints one entry per file.
- **`--detect`** parses only the loader (nothing is run, not even in the sandbox) and prints one line per file:
  `name`, `bacon_guard`, `moonveil` or `unknown`, a confidence and the build. `1.00` means the loader layout is recognized, `0.50`
  means the banner is there but the layout is not (damaged, truncated, or a newer build), `0.00` means it is not a
  Bacon Guard / FORGE script. A `1.00` does not promise a full result: the handlers of the build are only checked
  when the script is really deobfuscated. A MoonVeil script (found by its banner) counts as recognized: it will be traced,
  not decompiled. The exit code is 2 if any file is not recognized.

### Naming variables (`annotate` + names file)
`--annotate` (the `annotate` option of `/deobf`) writes a key after every variable, for example
`local Players<@12:34> = game:GetService("Players")`. That copy is **not runnable Luau**: it is only there so you can
read the keys and choose names. Put your names in a JSON file such as `{"12:34": "playerList"}` and run again with
`--names` (the `names` option of `/deobf`) to get clean code. The annotated copy is saved as `<name>_annotated.lua`
(the clean one as `<name>_deobf.lua`), and the syntax/bytecode checks always run on a clean copy of the same code.

## Fake Roblox sandbox (`--trace` and `--sandbox`)
Runs a script in a pretend Roblox and writes down what it does. Nothing real is reached: no network, no files, no real
game, and time is simulated (`task.wait(1000)` takes no real time).

```
python -m bgdeobf script.lua --trace       # deobfuscate, then run the RESULT        -> script_deobf.lua + script_trace.txt
python -m bgdeobf script.lua --sandbox     # run the file AS GIVEN, no deobfuscation -> script_trace.txt
python -m bgdeobf script.lua --trace --explore --responses answers.json --username Alwi --place-id 1234
```

**What is written down** (in the order it happened, each with the simulated time and the line of the script):
- every address the script asks for (`HttpGet`, `HttpPost`, `request`, WebSockets ...). Nothing is sent; the sandbox answers
  (an empty text unless `--responses` gives one);
- every `loadstring`: the code it was given is kept and then run, so a loader that fetches a loader unwraps itself;
- remotes fired (`FireServer`, `InvokeServer`), windows and other instances created, events connected, `Kick`, teleports,
  `queue_on_teleport`, files read and written (in a virtual folder), executor functions used (`hookfunction`,
  `setclipboard` ...), everything printed, every error;
- what the script asks about its surroundings (`game.PlaceId`, `identifyexecutor()`, a function that does not exist) and
  the answer it got.

**What you get:** `<name>_trace.txt` and one `<name>_payload<N>.<ext>` per captured text (code given to `loadstring`,
request bodies, files written, code queued for a teleport, long clipboard texts). The report has a header (size, SHA-256,
why the run ended), a **SUMMARY** (network, captured texts, remotes, files, executor functions, windows), **WORTH A LOOK
FIRST** (plain findings such as "Downloads code and runs it", "Sends data to a Discord webhook", "Kicks the player"; they
say what the script did in this run, **not** whether it is malicious), the **LOG**, and **LIMITS OF THIS TRACE**.
Repeats (a loop that fires the same remote 5000 times) are shown a few times and then counted.

| Option | Meaning |
|---|---|
| `--trace` | deobfuscate first, then run the result (lines in the log are lines of the deobfuscated file) |
| `--sandbox` | do not deobfuscate, run the file as it is (lines are lines of the file you gave). Works for any Luau script |
| `--explore` | afterwards call the handler of every button the script made (up to 60), one after another |
| `--responses FILE` | JSON `{"https://example.com/a.lua": "print(1)", "https://api.example.com/*": "..."}`: what `HttpGet` / `request` answer. A URL ending in `*` matches a prefix |
| `--place-id`, `--user-id`, `--username`, `--executor`, `--players` | what the pretend game says about itself |
| `--sandbox-time S` | simulated seconds to run (default 30) |
| `--sandbox-timeout S` | real seconds before the run is stopped (default 30) |

The run ends when every thread has finished or waits for something that never happens; when only frame handlers
(`Heartbeat` ...) are left after 10 simulated seconds; at the simulated time limit; at a limit on thread wake-ups
(200000), log lines (8000), memory (512 MB) or output (64 MB); or at the real time limit. The report says which. Whatever
was written before a stop is kept.

In the bot: `/deobf ... trace:True` or `sandbox:True`, in chat `!uwu trace` or `!uwu sandbox` (with the attachment or the
URL as usual). The report and the captured texts come back as files (as a zip when there are several). The message in
Discord gives counts (requests, remotes, windows ...), the host names of the requests when they are plain valid names
(in code spans, so they do not become links), and the titles of the findings; nothing else taken from the script is
repeated there, it is all in the report file.

**Safety.** The deobfuscation never runs the program inside the VM. `--trace` and `--sandbox` **do run the script's code**,
so:
- It runs in a separate `luau` process with no standard input, no environment variables (only a bare `PATH`), its own
  session and process group, an empty temporary folder, limits on memory, CPU time, file size, open files and output, and
  it is killed (with everything it started) at the time limit.
- Inside, the script only sees the pretend world. `io`, `os.execute`, `package` and `load` do not exist; `readfile`,
  `writefile`, `dofile`, `request`, `loadstring`, `require` and the other functions of an executor are fakes that only
  write to the log and to a virtual folder. The runtime reads no global after its environment is emptied (checked on the
  compiled code).
- The tests throw a hostile script at it (`os.execute`, `io.open`, `require("/etc/passwd")`, `getfenv` and coroutine
  tricks, `writefile("/tmp/...")`, real sockets, printing fake log lines, endless loops, memory bombs). Nothing real was
  reached, nothing was written, the log could not be forged, and under `strace` the process made no network, process or
  write system calls.
- It is still a process on your machine, not a hardened jail (no seccomp, no namespaces): a bug in Luau itself could in
  theory get out. For a bot that other people can use, run it in a separate VM or container without network access, as
  said under *Limits and safety*.

**Limits.**
- It is a pretend Roblox, so the answers are approximations. Services answer with made-up values, and a script that needs a
  real game, other players or an answer from a server may stop early or never reach the interesting part. The "Ended" line
  and the errors in the log say where it stopped.
- When a script reads something the sandbox does not know, it gets a stand-in value that accepts almost anything and
  writes the question to the log. A stand-in compared with a number (`x > 5`) or used as a loop limit (`for i = 1, x`)
  still raises an error, because Luau forbids that comparison; the script then stops there and the error is logged.
- A script can notice it is not in a real game (for example from the text of some `pcall` errors, which mention
  `./sandbox.lua`) and behave differently. A clean trace is not proof that a script is harmless.
- Nothing is downloaded, so code a real server would send is only seen when `--responses` provides it.
- `--sandbox` on a protected script runs the VM script itself: it works, but the lines in the log are lines of the
  protected file. Use `--trace` to see lines of the readable result.
- Without a pseudo terminal (Windows) the log of a run that had to be killed can miss its last few lines.

## MoonVeil (behaviour reconstruction, not source)
MoonVeil compiles a script into its own private VM program, and nothing in the file says what the original statements
were, so **no tool can give you the source back**. What can be done honestly is to run the script once against recording
fake Roblox objects and write down what it did. That is what you get for a MoonVeil script, in `<name>_reconstructed.lua`:

- The file starts with a comment that says, in capitals, that it is **not** the original source, what it is, and what was
  and was not observed. It also lists what stands out: code the script downloads and runs, remotes it fires, texts it
  copies to the clipboard, executor-only functions, names it reads but never defines, and other links in its strings.
- Below that are the calls the script made, in order, as Lua-like code with the real names, strings and numbers.
  Loops are folded. A place the run could not see (the other branch of a condition, the answer of a server, code that is
  downloaded at run time) is marked `UNKNOWN`. Local names (`Players`, `gui` ...) were chosen by this tool.
- Not recovered: the structure of the original code, its conditions, its variable names, everything that did not run in
  the sandbox, and the downloaded code (nothing is fetched).
- Threads and callbacks run one after another, so the real timing between them is not kept. Loops are cut after 40
  waits, and the trace stops after 8000 events; the result says so when something was cut, or when the script stopped
  with an error. The syntax of the result is checked with `luau-compile`.

**How a script is taken for MoonVeil.** The first 1500 characters contain the banner
`-- This script was generated using MoonVeil vX` or the text `moonveil.cc`. The CLI, `/deobf` and `!uwu` do that by
themselves. **`!moonveil`** asks for it explicitly, so it also works when the banner was removed (the result then says
that no banner was found). A script that carries a Bacon Guard banner is refused there: use `!uwu` for it. Add `trace`
(`--trace` on the command line) to also get the fake Roblox report of the script as given, with the same
`<name>_trace.txt` and captured texts as described above.

**Safety.** Making the reconstruction **runs the script's code**, in the same limited Luau process that unpacks a VM (no
`io`, no network, limits on memory, CPU time and output) against a recording stand-in for Roblox. The harness keeps its
own variables and the real globals out of the script's reach (there is a test for that), but it is not a hardened jail;
the advice under *Limits and safety* applies to it as much as to `trace`.

## Discord bot
1. discord.com/developers/applications → New Application → Bot → Reset Token → copy the token.
2. Still on the Bot page, turn on **MESSAGE CONTENT INTENT** (Privileged Gateway Intents). It is only needed for
   `!uwu` and `!moonveil`; without it set `PREFIX_COMMANDS=0` in `.env` and use slash commands only.
3. `cp .env.example .env` and fill in `DISCORD_TOKEN=`. (Never share `.env`.)
4. `python -m bgdeobf.bot`
5. Invite the bot: OAuth2 → URL Generator → scopes `bot` + `applications.commands`. Permissions `!uwu` and `!moonveil` use:
   View Channel, Send Messages, Attach Files, Read Message History. Slash commands need no channel permissions.

How to use it:

| Command | What it does |
|---|---|
| `!uwu` + attachment | deobfuscates the attached `.lua` / `.luau` / `.txt` file; the result is a reply in the channel |
| `!uwu <raw url>` | downloads from the URL, then deobfuscates. You can also paste the whole line: `!uwu loadstring(game:HttpGet("https://…"))()` |
| reply to a message with `!uwu` | uses the file or URL in the message you replied to |
| `/deobf file:…` or `/deobf url:…` | same, but only you can see the result (`private`, on by default); options `annotate`, `names`, `trace`, `sandbox`. A MoonVeil script is found by its banner and gets the behaviour reconstruction |
| `!moonveil` + attachment, `!moonveil <raw url>`, or a reply with `!moonveil` | for a MoonVeil script: the behaviour reconstruction (`<name>_reconstructed.lua`, **not** source), also when its banner is missing. Add `trace` for the fake Roblox report too. Not for Bacon Guard scripts: use `!uwu` |
| `!uwu trace` / `!uwu sandbox` + attachment or URL | also run the result in the fake Roblox (`trace`), or run the file as it is without deobfuscating (`sandbox`); the report comes back as a file |
| right-click a message → Apps → **Deobfuscate file** | uses the attachment or URL in that message (private result) |
| `/deobf-info`, `!uwu help`, `!moonveil help` | help |

GitHub `blob` links, normal Pastebin links and Gists are converted to their raw form automatically. Links to web pages
(HTML) are rejected with a hint.
`!uwu` and `!moonveil` reply in the channel, so everyone there can see the result; use a slash command if you want it private.

While a job runs the bot shows a **progress card**: one message with the steps (validating and downloading a URL, detecting
the obfuscator with the stage the engine is in, preparing the download), a spinner and the elapsed time. It turns green
(or red, with the reason in the message) when the job ends and stays on the final message. `PROGRESS_CARD=0`
switches it off and a plain-text status line is used instead.

Settings in `.env` (all optional except the token):

| Name | Default | Meaning |
|---|---|---|
| `GUILD_ID` | empty | register the slash commands to one server only (they appear immediately, good for testing) |
| `ALLOWED_GUILDS`, `ALLOWED_USERS` | empty = everyone | restrict who can use the bot (IDs separated by commas); applies to slash commands and `!uwu` |
| `PREFIX`, `PREFIX_COMMANDS` | `!`, 1 | prefix of the chat commands; `0` turns `!uwu` and `!moonveil` off (and then the Message Content Intent is not needed) |
| `URL_HOSTS` | empty = any public host | limit URLs to certain domains, e.g. `raw.githubusercontent.com, pastebin.com` |
| `MAX_MB` | 3 | maximum file size (attachments and URLs) |
| `WORKERS`, `MAX_QUEUE` | 2, 8 | parallel jobs and queue length |
| `JOB_TIMEOUT` | 240 | hard time limit per job (seconds); the process is killed when it runs over |
| `COOLDOWN` | 15 | pause per user (seconds) |
| `PROGRESS_CARD` | 1 | `0` = no live progress card, a plain-text status line instead |
| `SPINNER_EMOJI` | empty | an animated custom emoji for the card's spinner, written like `<a:spin:123456789012345678>` (the bot has to be able to use it); empty = a text spinner |

One user can only have one job running at a time.

## Limits and safety
- Every job runs in a separate process with a memory limit and a hard time limit; the Luau sandbox has CPU, memory and
  output limits and no `io`. When a job finishes or is cancelled, all its processes and its temporary folder are cleaned up.
- URL downloads are restricted so nobody can use the bot to reach your server's network: only `http`/`https` on
  port 80/443, no `user:password`, **every** IP address returned by DNS must be public (loopback, private networks,
  link-local/cloud metadata, local IPv6 and IPv4 addresses wrapped in IPv6 are rejected; the check lives in the resolver
  the connection uses, so there is no DNS-rebinding gap), every redirect is checked again (max 3), size limits (also
  after decompression) and time limits apply, no cookies are sent, and proxies from the environment are not used.
- `trace`, `sandbox` and the MoonVeil reconstruction (`!moonveil`, or a MoonVeil banner) run the script's code (in a fake Roblox,
  in a separate limited process; see *Safety* above). Anyone who can use the bot can use them, so restrict the bot with `ALLOWED_GUILDS` / `ALLOWED_USERS` when it is not private, and
  run it where it has no network access to your other machines.
- Files are processed in memory and a temporary folder; the log records only the user id, the size and the outcome, never
  the contents. Messages shown in Discord are scrubbed of file paths from the machine running the bot.
- For a bot that is open to the public, run it in a separate VM/container and use `ALLOWED_GUILDS`.
- If the script is neither Bacon Guard nor MoonVeil, is truncated, is too large, or the build uses handler patterns that
  are not recognized yet, the result is a **clear error**, not a guess. (A MoonVeil script is not guessed at either: it gets
  the labelled reconstruction and nothing that looks like source.)

## Luau binaries
`luau_bin/` holds the official Luau binaries (`luau`, `luau-ast`, `luau-compile`; MIT license, see
`luau_bin/LICENSE-luau.txt`). The bundled ones are for Linux x86-64 (glibc 2.34+, e.g. Ubuntu 22.04+). On another OS,
download `luau`, `luau-ast` and `luau-compile` from the official luau-lang/luau releases, put them in one folder and
set `BGDEOBF_LUAU_DIR=<that folder>`. On Windows/macOS the sandbox memory limit is not active (the time limit still is).
`python -m bgdeobf --check` shows whether the tools can run.

## What has and hasn't been tested
- Tested on 11 real builds (Bacon Guard v0.42.0 "The Morgue Shift", ClarksCicaverse, TommyPlay, FORGE Protection build
  03d3bec6a6b0, a build with no banner that keeps the program in compressed pages, and Bacon Guard v0.43.0 | FORGE
  Protection builds baf6ea30ebd0, cf776ebed029, a7c98b93da5b, af206410f1dd, cb5687cb3df4 and d0fca08c7b7b). Bytecode
  check, in that order: 168/168, 24/24, 172/172, 93/93, 172/172, 70/70, 62/62, 77/77, 58/58, 85/85 and 49/49 functions
  identical, meaning the result compiles back to the same instructions in the same order (registers are not compared,
  since they can differ for the same source). On another build the check can still report a function as "reordered"
  (same instructions, a plain read or calculation in a different place) or "cosmetic" (one `return` more or less); the
  summary says so.
  `--detect` and a whole-folder run with `--jobs 3` were also tried on the first 10 of those scripts: all 10 were
  recognized and all 10 results came out identical.
  Other builds may use patterns that are not covered yet; read the error message. The protector changes small details
  between builds (for example how the loader ends), so a new build can fail with a clear error message until that
  detail is added.
- A second check that the bytecode check cannot do: the protected script and its deobfuscated result are both run in the
  fake Roblox and **every** event they log is compared (only line numbers inside function descriptions and clock
  readings with many decimals are ignored). On all 11 builds the two logs are the same, except for one flag that the
  protection layer sets for itself (`getgenv()._xx = 0/1`) and the result does not have. This matters because the bytecode
  check compares the result with the *same decode*, so it can not see something the decode lost. This second check
  found exactly that: in four builds (cf776ebed029, a7c98b93da5b, cb5687cb3df4 and d0fca08c7b7b) the handler that builds a
  table from a template was taken for the one that builds an empty table, so every table field whose value is a constant
  was missing from the result (settings such as `AutoLobby = true`, `__mode = "v"`, `__metatable = "locked"`). That is
  fixed. If you made a result of one of those builds with an earlier version, make it again. The fix has tests on a made-up
  handler (they fail without it), and the sandbox comparison runs on every protected script in `tests/samples/`.
- v0.43.0 changed how the VM stores a program: every function's registers are shifted by a per-function number, a loop
  can be one record that holds the whole loop, and a closure record carries its own capture list. All three are handled.
  Build d0fca08c7b7b no longer chooses between several record layouts per function: the loader spells out one layout and
  every function uses it. That is handled too (a loader with the usual layout ladder works as before).
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
  then `size.Y * 0.5`): it is kept as a local, because writing it into the expression would move its read. A value computed
  before the table existed (`local v = a and b or c` and then `{k = v}`) is kept as a local too, because the compiler
  creates a table first and computes what goes into it afterwards.
- The result has a blank line around every block (function, `if`, loop) and between two short
  `if ... then return end` guards, so long scripts read more easily. It changes nothing in the program.
- About 470 automated tests (`python -m pytest -q tests/`; the ones that need a protected script are skipped when
  `tests/samples/` is empty): sandbox limits, damaged/truncated files, hard timeout, queue, the URL downloader (local
  server + fake resolver), the command line (single file, folders, `--jobs`, `--detect`), the decompiler and the decoder
  on small hand-assembled programs (these need no protected script), the fake Roblox (events, limits, hostile scripts),
  MoonVeil, the progress card, and the whole bot flow (slash, menu, `!uwu`, `!moonveil`, `trace`, `sandbox`)
  using the **real discord.py library with a fake Discord network**.
  A live connection to Discord could **not** be tested where this was built (no token), so the progress card, `!moonveil`
  and the `trace` / `sandbox` replies have not been seen on a real server. To check on your machine: set `GUILD_ID`, start
  the bot, send one file with `/deobf`, one with `!uwu` and one with `!moonveil`. Downloading from the real internet (e.g.
  raw.githubusercontent.com) could not be tried there either; the TLS path uses aiohttp's defaults.
- MoonVeil was tested with readable scripts that behave like protected ones (calls, loops, callbacks, downloads, a script
  that tries to reach the real globals) in the real Luau sandbox, and with logs made up to test the reader. A real
  MoonVeil-protected file could not be tried where this was built, so a real one may show gaps; read the result as what it
  is, a record of one run. The same goes for the fake Roblox: it is an approximation that was never compared with a
  real game; what was checked is that a protected script and its deobfuscated result behave the same way in it (the
  11 scripts above).
- A build that stores table entries under a constant number (`t[1] = v`, the `SETTABLEN` instruction) is handled by the
  decoder, the decompiler, the handler check and the bytecode comparison; a test on two hand-assembled programs covers it.
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
| `bot.py` | Discord bot (slash, menu, `!uwu`, `!moonveil`) |
| `card.py` | the live progress card the bot shows while a job runs |
| `fetch.py` | safe URL downloader (anti-SSRF) |
| `loader.py`, `patch.py`, `dumper.lua.tpl`, `sandbox.py` | recognize the loader, unpack the VM; `sandbox.py` also runs the limited Luau process |
| `moonveil.py`, `moonveil.lua.tpl` | MoonVeil: runs the script in a recording harness and writes the behaviour reconstruction |
| `tracer.py` | runs a script in the fake Roblox, reads its log, writes the report (`--trace`, `--sandbox`) |
| `lua/*.lua` | the pretend Roblox itself, in parts (log, scheduler, types, instances, services, executor functions) |
| `classify.py`, `luavm.py`, `refspec.py` | recognize the handlers of each build |
| `decode.py`, `decomp.py`, `gen.py`, `post.py`, `printer.py`, `render.py`, `fidelity.py` | decompiler and verification |
| `messages.py` | the texts shown to users (and the path scrubbing) |

## License
The code is under the Apache License 2.0 (`LICENSE`). The Luau binaries in `luau_bin/` are MIT licensed
(`luau_bin/LICENSE-luau.txt`).
