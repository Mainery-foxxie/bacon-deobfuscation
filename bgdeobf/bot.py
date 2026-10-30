"""Discord bot.

  /deobf file:<attachment>  or  /deobf url:<raw url>
  !uwu  (+ attachment)   |   !uwu <raw url>   |   reply to a message that has a file/URL with !uwu
  right-click a message -> Apps -> "Deobfuscate file"

Run:  python -m bgdeobf.bot     (token in .env: DISCORD_TOKEN=...)"""
import asyncio
import io
import logging
import os
import re
import sys
import time
import zipfile

try:
    import aiohttp
    import discord
    from discord import app_commands
except ImportError:
    raise ImportError('discord.py is not installed. Run:  pip install -r requirements.txt')

from .astutil import check_tools
from .card import Card
from .engine import EngineError, Options
from .fetch import FetchError, check_url, extract_url, fetch, normalize_url, url_stem
from .messages import explain, explain_fetch, verdict
from .namesfile import NamesError, parse_names
from .service import Busy, Service

log = logging.getLogger('bgdeobf.bot')

SCRIPT_EXT = ('.lua', '.luau', '.txt')
NAMES_EXT = ('.json', '.txt')
NAMES_MAX_BYTES = 256 * 1024
DM_UPLOAD_LIMIT = 10 * 1024 * 1024
CARD_INTERVAL = 1.2     # seconds between redraws of the progress card (moves the spinner and the timer)
COMMAND = 'uwu'
NOTE = 'Variable names are automatic guesses. Only analyze scripts you own or are allowed to analyze.'

STAGES = {
    'analyse': 'reading the loader',
    'unpack': 'opening the VM in the sandbox',
    'classify': "recognizing this build's handlers",
    'decompile': 'rebuilding the code',
    'verify': 'verifying the result',
}

INFO = (
    '**Bacon Guard / FORGE deobfuscator**\n'
    '• `/deobf file:<attachment>` or `/deobf url:<raw url>`\n'
    '• `{p}uwu` + attachment, `{p}uwu <raw url>`, or reply to a message that has a file/URL with `{p}uwu`{chat_off}\n'
    '• Right-click a message with an attachment → Apps → **Deobfuscate file**\n'
    '• Formats: .lua / .luau / .txt, max {mb:g} MB. GitHub `blob`, Pastebin and Gist links are converted to raw automatically\n'
    '• Takes about 5–60 seconds; the result is re-checked (syntax + bytecode)\n'
    '• Builds that are not recognized are rejected with a clear reason, never guessed\n'
    '• `annotate` writes a key next to every variable; fill in a JSON file `{{"12:34": "name"}}` and send it via `names`\n'
    '• Slash commands: the result is visible only to you by default (`private`). `{p}uwu` replies in the channel\n'
    '-# Use only on scripts you own or are allowed to analyze.'
)

CHAT_HELP = ('Usage: `{p}uwu` + a .lua attachment, or `{p}uwu <raw url>`. You can also reply to a message that has a file/URL with `{p}uwu`. '
             'Example: `{p}uwu https://raw.githubusercontent.com/user/repo/main/script.lua`')


def _ids(text):
    out = set()
    for part in re.split(r'[,\s]+', text or ''):
        if part:
            out.add(int(part))
    return out


def _flag(text, default):
    if text is None or str(text).strip() == '':
        return default
    return str(text).strip().lower() not in ('0', 'false', 'no', 'off', 'tidak')


class Config(object):
    def __init__(self, env=None):
        e = os.environ if env is None else env
        try:
            self.token = (e.get('DISCORD_TOKEN') or '').strip()
            self.guild_id = int(e['GUILD_ID']) if e.get('GUILD_ID') else None
            self.allowed_guilds = _ids(e.get('ALLOWED_GUILDS'))
            self.allowed_users = _ids(e.get('ALLOWED_USERS'))
            self.max_mb = float(e.get('MAX_MB') or 3)
            self.workers = int(e.get('WORKERS') or 2)
            self.max_queue = int(e.get('MAX_QUEUE') or 8)
            self.timeout = int(e.get('JOB_TIMEOUT') or 240)
            self.cooldown = float(e.get('COOLDOWN') or 15)
            self.prefix = (e.get('PREFIX') or '!').strip() or '!'
            self.prefix_commands = _flag(e.get('PREFIX_COMMANDS'), True)
            self.progress_card = _flag(e.get('PROGRESS_CARD'), True)
            self.url_hosts = set(h.strip().strip('.').lower() for h in re.split(r'[,\s]+', e.get('URL_HOSTS') or '') if h.strip())
            self.url_timeout = 25
            self.max_wait = 13 * 60.0
        except ValueError as ex:
            raise ValueError('invalid value in .env: %s' % ex)
        if not (0 < self.max_mb <= 8 and 1 <= self.workers <= 16 and 0 <= self.max_queue <= 100 and 30 <= self.timeout <= 900):
            raise ValueError('MAX_MB (0-8), WORKERS (1-16), MAX_QUEUE (0-100), JOB_TIMEOUT (30-900) out of range')
        if not (1 <= len(self.prefix) <= 3) or any(c.isspace() for c in self.prefix):
            raise ValueError('PREFIX must be 1-3 characters with no spaces')
        for h in self.url_hosts:
            if not re.fullmatch(r'[a-z0-9.-]{1,253}', h):
                raise ValueError('URL_HOSTS contains an invalid host name: %r' % h)

    @property
    def max_bytes(self):
        return int(self.max_mb * 1024 * 1024)


def load_dotenv(path='.env', environ=None):
    environ = os.environ if environ is None else environ
    try:
        with open(path, encoding='utf-8') as f:
            lines = f.read().splitlines()
    except OSError:
        return False
    for line in lines:
        line = line.strip()
        if not line or line.startswith('#') or '=' not in line:
            continue
        if line.startswith('export '):
            line = line[7:]
        k, v = line.split('=', 1)
        v = v.strip()
        if len(v) >= 2 and v[0] == v[-1] and v[0] in '"\'':
            v = v[1:-1]
        environ.setdefault(k.strip(), v)
    return True


def safe_stem(filename):
    stem = os.path.splitext(os.path.basename(filename or 'script'))[0]
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', stem).strip('._-')[:60]
    return stem or 'script'


def parse_command(content, prefix, command=COMMAND):
    """'!uwu <args>' -> '<args>' (may be ''); None when the message is not that command"""
    text = (content or '').lstrip()
    head = prefix + command
    if text[:len(head)].lower() != head.lower():
        return None
    rest = text[len(head):]
    if rest and not rest[0].isspace():
        return None
    return rest.strip()


# ---------------------------------------------------------------------------------------------- where answers go
class Responder(object):
    """The place the answers of one job go: an interaction (slash command / menu) or a chat message."""
    user_id = 0
    guild_id = None
    upload_limit = DM_UPLOAD_LIMIT

    async def refuse(self, text):
        raise NotImplementedError

    async def accept(self):
        pass

    async def status(self, text):
        raise NotImplementedError

    async def show(self, card):
        """draw (or redraw) the live progress card"""
        raise NotImplementedError

    async def finish(self, text, attachment=None, card=None):
        """attachment: (filename, bytes) or None; card: the finished progress card to leave on screen, or None to clear it"""
        raise NotImplementedError


class InteractionResponder(Responder):
    def __init__(self, interaction, private=True):
        self.i = interaction
        self.private = bool(private)
        self.user_id = interaction.user.id
        self.guild_id = getattr(interaction, 'guild_id', None)
        guild = getattr(interaction, 'guild', None)
        self.upload_limit = (getattr(interaction, 'filesize_limit', None) or getattr(guild, 'filesize_limit', None)
                             or DM_UPLOAD_LIMIT)

    async def refuse(self, text):
        await self.finish(text)

    async def accept(self):
        await self.i.response.defer(ephemeral=self.private, thinking=True)

    async def status(self, text):
        try:
            await self.i.edit_original_response(content=text[:1900])
        except discord.HTTPException:
            pass

    async def show(self, card):
        try:
            await self.i.edit_original_response(content=None, embeds=[card.embed()])
        except discord.HTTPException:
            pass

    async def finish(self, text, attachment=None, card=None):
        text = text[:1900]
        f = discord.File(io.BytesIO(attachment[1]), filename=attachment[0]) if attachment else None
        if self.i.response.is_done():
            kw = {'content': text, 'embeds': [card.embed()] if card is not None else []}
            if f is not None:
                kw['attachments'] = [f]
            await self.i.edit_original_response(**kw)
        else:
            kw = {'content': text, 'ephemeral': True}
            if f is not None:
                kw['file'] = f
            await self.i.response.send_message(**kw)


class MessageResponder(Responder):
    def __init__(self, message):
        self.m = message
        self.user_id = message.author.id
        guild = getattr(message, 'guild', None)
        self.guild_id = guild.id if guild is not None else None
        self.upload_limit = getattr(guild, 'filesize_limit', None) or DM_UPLOAD_LIMIT
        self.status_msg = None

    async def _send(self, text, attachment=None, card=None):
        kw = {'mention_author': False, 'allowed_mentions': discord.AllowedMentions.none()}
        text = text[:1900] if text else None
        for with_reference in (True, False):
            if attachment:
                kw['file'] = discord.File(io.BytesIO(attachment[1]), filename=attachment[0])
            if card is not None:
                kw['embed'] = card.embed()
            try:
                if with_reference:
                    return await self.m.reply(text, **kw)
                kw.pop('mention_author', None)
                return await self.m.channel.send(text, **kw)
            except discord.Forbidden:
                raise
            except discord.HTTPException:
                if not with_reference:
                    raise

    async def refuse(self, text):
        await self._send(text)

    async def status(self, text):
        try:
            if self.status_msg is None:
                self.status_msg = await self._send(text)
            else:
                await self.status_msg.edit(content=text[:1900])
        except discord.HTTPException:
            pass

    async def show(self, card):
        try:
            if self.status_msg is None:
                self.status_msg = await self._send(None, card=card)
            else:
                await self.status_msg.edit(content=None, embeds=[card.embed()])
        except discord.HTTPException:
            pass

    async def finish(self, text, attachment=None, card=None):
        if self.status_msg is not None:
            try:
                kw = {'content': text[:1900], 'embeds': [card.embed()] if card is not None else []}
                if attachment:
                    kw['attachments'] = [discord.File(io.BytesIO(attachment[1]), filename=attachment[0])]
                await self.status_msg.edit(**kw)
                return
            except discord.HTTPException:
                pass
        try:
            await self._send(text, attachment, card)
        except discord.Forbidden:
            if attachment:
                await self._send("I don't have the **Attach Files** permission in this channel, so I can't send the result.")
            else:
                raise


# ---------------------------------------------------------------------------------------------- the logic
class Handler(object):
    """Everything the commands do; works on Responder objects, so it needs no decorators and is easy to test."""

    def __init__(self, cfg, service):
        self.cfg = cfg
        self.service = service
        self._last = {}
        self._inflight = set()
        self._fetching = None

    # ------------------------------------------------------------ checks
    def allowed(self, r):
        c = self.cfg
        if not c.allowed_guilds and not c.allowed_users:
            return True
        if r.user_id in c.allowed_users:
            return True
        return r.guild_id is not None and r.guild_id in c.allowed_guilds

    def cooldown_left(self, uid):
        t = self._last.get(uid)
        if t is None:
            return 0
        return max(0.0, self.cfg.cooldown - (time.monotonic() - t))

    def _stamp(self, uid):
        now = time.monotonic()
        if len(self._last) > 2000:
            self._last = dict((k, v) for k, v in self._last.items() if now - v < self.cfg.cooldown)
        self._last[uid] = now

    def check_attachment(self, att, names=False):
        name = (att.filename or '').lower()
        exts = NAMES_EXT if names else SCRIPT_EXT
        limit = NAMES_MAX_BYTES if names else self.cfg.max_bytes
        if not name.endswith(exts):
            return 'Send a %s file.' % ' / '.join(exts)
        if att.size == 0:
            return 'The file is empty.'
        if att.size > limit:
            return 'The file is too large (%.1f MB, max %s).' % (
                att.size / 1048576.0, '256 KB' if names else '%g MB' % self.cfg.max_mb)
        return None

    def check_source_url(self, url):
        """(normalized url, None) or (None, message) - no network involved"""
        try:
            url = normalize_url(url)
            check_url(url, self.cfg.url_hosts or None)
        except FetchError as e:
            return None, explain_fetch(e.code, e.message, self.cfg.max_mb)
        return url, None

    # ------------------------------------------------------------ the job
    async def process(self, interaction, file=None, names_file=None, private=True, annotate=False, url=None):
        """slash command / menu entry"""
        return await self.run(InteractionResponder(interaction, private), file=file, url=url, names_file=names_file,
                              annotate=annotate)

    async def run(self, r, file=None, url=None, names_file=None, annotate=False):
        uid = r.user_id
        if not self.allowed(r):
            return await r.refuse("This bot isn't open to this server/account.")
        if uid in self._inflight:
            return await r.refuse('You already have a file being processed. Wait until it finishes.')
        wait = self.cooldown_left(uid)
        if wait > 0:
            return await r.refuse('Please wait %d s before sending the next file.' % (int(wait) + 1))
        if file is None and not url:
            return await r.refuse('Include a .lua / .luau / .txt file or a raw URL.')
        if file is not None and url:
            return await r.refuse('Pick one: a file or a URL, not both.')
        problem = None
        if file is not None:
            problem = self.check_attachment(file)
        else:
            url, problem = self.check_source_url(url)
        if problem is None and names_file is not None:
            problem = self.check_attachment(names_file, names=True)
        if problem:
            return await r.refuse(problem)
        self._inflight.add(uid)
        t0 = time.monotonic()
        outcome = 'ok'
        source = 'file' if file is not None else 'url'
        card = pump = None
        try:
            await r.accept()
            if self.cfg.progress_card:
                # the card is drawn by one background task only, so edits never overlap; the steps below just change its state
                card = Card(from_url=file is None)
                card.begin('detect' if file is not None else 'download')
                pump = asyncio.ensure_future(self._pump(r, card))

            async def end(text, attachment=None, ok=False):
                """stop the card, mark it done/failed, and put the final message on screen"""
                if pump is not None:
                    await self._stop_pump(pump)
                if card is not None:
                    card.complete() if ok else card.fail()
                return await r.finish(text, attachment, card=card)

            try:
                if file is not None:
                    data = await file.read()
                    stem = safe_stem(file.filename)
                else:
                    if card is None:
                        await r.status('⬇️ Downloading from the URL…')
                    if self._fetching is None:
                        self._fetching = asyncio.Semaphore(4)
                    async with self._fetching:
                        data = await fetch(url, self.cfg.max_bytes, hosts=self.cfg.url_hosts or None,
                                           timeout=self.cfg.url_timeout)
                    stem = safe_stem(url_stem(url))
                    if card is not None:
                        card.begin('detect')
                names = parse_names(await names_file.read()) if names_file is not None else None
            except FetchError as e:
                outcome = 'fetch:' + e.code
                return await end(explain_fetch(e.code, e.message, self.cfg.max_mb))
            except NamesError as e:
                outcome = 'bad_names'
                return await end(str(e))
            except (discord.HTTPException, OSError):
                outcome = 'download'
                return await end('Failed to download the attachment from Discord. Try sending it again.')
            if len(data) > self.cfg.max_bytes:
                outcome = 'too_big'
                return await end('The file is too large (max %g MB).' % self.cfg.max_mb)
            last_edit = [0.0]

            async def progress(ev):
                kind, val = ev
                if card is not None:
                    if kind == 'queued':
                        card.queued(val)
                    else:
                        card.stage(val)
                    return
                now = time.monotonic()
                if kind == 'queued':
                    text = '⏳ Queued (position %d)…' % val
                else:
                    text = '⚙️ Processing: %s…' % STAGES.get(val, val)
                if now - last_edit[0] < 1.0 and kind != 'queued':
                    return
                last_edit[0] = now
                await r.status(text)

            try:
                res = await asyncio.wait_for(
                    self.service.submit(uid, data, names=names, annotate=annotate, progress=progress), self.cfg.max_wait)
            except asyncio.TimeoutError:
                outcome = 'wait_timeout'
                return await end('The queue took too long and the job was cancelled. Try again later.')
            except Busy as b:
                outcome = b.reason
                return await end('You already have a file being processed. Wait until it finishes.'
                                 if b.reason == 'user_busy' else 'The queue is full. Try again in a few minutes.')
            except EngineError as e:
                outcome = 'error:' + e.code
                if e.code == 'internal':
                    log.error('internal error: %s', e.message)
                if e.code == 'too_big':
                    return await end('The file is too large (max %g MB).' % self.cfg.max_mb)
                return await end(explain(e.code, e.message))
            if card is not None:
                card.begin('deliver')
            return await self.deliver(r, stem, res, time.monotonic() - t0, end=end)
        except discord.HTTPException as e:
            outcome = 'discord_error'
            log.warning('discord error while replying: %s', e)
        except Exception:
            outcome = 'crash'
            log.exception('unexpected failure')
            try:
                if pump is not None:
                    await self._stop_pump(pump)
                if card is not None:
                    card.fail()
                await r.finish(explain('internal'), card=card)
            except Exception:
                pass
        finally:
            if pump is not None:
                pump.cancel()
            self._inflight.discard(uid)
            self._stamp(uid)
            log.info('job user=%s guild=%s src=%s size=%s -> %s %.1fs', uid, r.guild_id, source,
                     getattr(file, 'size', '-'), outcome, time.monotonic() - t0)

    async def _pump(self, r, card):
        """the only writer of the progress card: draws it, waits a moment, moves the spinner, draws it again"""
        try:
            while True:
                await r.show(card)
                await asyncio.sleep(CARD_INTERVAL)
                card.tick()
        except asyncio.CancelledError:
            raise
        except Exception:
            log.exception('progress card stopped')

    @staticmethod
    async def _stop_pump(pump):
        pump.cancel()
        try:
            await pump
        except (asyncio.CancelledError, Exception):
            pass

    async def deliver(self, r, stem, res, seconds, end=None):
        if end is None:
            async def end(text, attachment=None, ok=False):
                return await r.finish(text, attachment)
        body = res.text.encode('utf-8')
        out_name = stem + ('_annotated.lua' if res.info.get('annotated') else '_deobf.lua')
        limit = r.upload_limit - 64 * 1024
        note = ''
        if len(body) > limit:
            buf = io.BytesIO()
            with zipfile.ZipFile(buf, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
                z.writestr(out_name, body)
            body = buf.getvalue()
            out_name += '.zip'
            note = '\nThe result was zipped because it exceeds the server upload limit.'
            if len(body) > limit:
                return await end('The result is too large to upload on this server (%d lines). '
                                 'Run it through the CLI on your computer.' % res.info.get('lines', 0))
        lines = ['✅ Done (%.0f s)' % seconds] + verdict(res.info)
        await end('\n'.join(lines) + note + '\n-# ' + NOTE, attachment=(out_name, body), ok=True)

    # ------------------------------------------------------------ entry points
    async def from_message(self, interaction, message):
        """context menu: first usable attachment of the clicked message, else a URL in its text"""
        r = InteractionResponder(interaction, True)
        for att in message.attachments:
            if (att.filename or '').lower().endswith(SCRIPT_EXT):
                return await self.run(r, file=att)
        url = extract_url(message.content)
        if url:
            return await self.run(r, url=url)
        return await r.refuse('That message has no .lua / .luau / .txt attachment or URL.')

    @staticmethod
    def can_reply(message):
        ch = message.channel
        guild = getattr(message, 'guild', None)
        if guild is None or not isinstance(ch, (discord.abc.GuildChannel, discord.Thread)):
            return True
        try:
            me = guild.me
            return me is None or ch.permissions_for(me).send_messages
        except Exception:
            return True

    @staticmethod
    async def _referenced(message):
        ref = message.reference
        if ref is None:
            return None
        if isinstance(ref.resolved, discord.Message):
            return ref.resolved
        if ref.message_id:
            try:
                return await message.channel.fetch_message(ref.message_id)
            except discord.HTTPException:
                return None
        return None

    def chat_help(self):
        return CHAT_HELP.format(p=self.cfg.prefix)

    async def from_chat(self, message, args):
        """!uwu: attachment, URL in the command, or the message being replied to"""
        r = MessageResponder(message)
        if not self.can_reply(message):
            return
        if args.strip().lower() in ('help', '?', 'bantuan'):
            return await r.refuse(self.chat_help())
        file = url = None
        for att in message.attachments:
            if (att.filename or '').lower().endswith(SCRIPT_EXT):
                file = att
                break
        if file is None and message.attachments:
            file = message.attachments[0]
        if file is None:
            url = extract_url(args)
        if file is None and url is None and message.reference is not None:
            ref = await self._referenced(message)
            if ref is not None:
                for att in ref.attachments:
                    if (att.filename or '').lower().endswith(SCRIPT_EXT):
                        file = att
                        break
                if file is None:
                    url = extract_url(ref.content)
        if file is None and url is None:
            return await r.refuse(self.chat_help())
        return await self.run(r, file=file, url=url)

    def info_text(self):
        return INFO.format(mb=self.cfg.max_mb, p=self.cfg.prefix,
                           chat_off='' if self.cfg.prefix_commands else ' (disabled on this bot)')


class DeobfClient(discord.Client):
    def __init__(self, cfg, service=None):
        intents = discord.Intents.none()
        intents.guilds = True
        if cfg.prefix_commands:
            intents.messages = True
            intents.message_content = True
        discord.Client.__init__(self, intents=intents)
        self.cfg = cfg
        self.service = service or Service(workers=cfg.workers, max_queue=cfg.max_queue,
                                          options=Options(max_bytes=cfg.max_bytes), hard_timeout=cfg.timeout)
        self.handler = Handler(cfg, self.service)
        self.tree = app_commands.CommandTree(self)
        self._register()

    def _register(self):
        tree, h = self.tree, self.handler

        @tree.command(name='deobf', description='Deobfuscate Roblox scripts protected with Bacon Guard / FORGE')
        @app_commands.describe(file='Protected .lua / .luau / .txt file',
                               url='Or the raw URL of the script (https://…)',
                               private='Only you can see the result (default: yes)',
                               annotate='Write a key next to every variable (to build a names file)',
                               names='JSON file {"key": "name"} used to name variables')
        async def deobf(interaction: discord.Interaction, file: discord.Attachment = None, url: str = None,
                        private: bool = True, annotate: bool = False, names: discord.Attachment = None):
            await h.process(interaction, file, names_file=names, private=private, annotate=annotate,
                            url=extract_url(url) or url if url else None)

        @tree.command(name='deobf-info', description='How to use it, limits, and usage notes')
        async def deobf_info(interaction: discord.Interaction):
            await interaction.response.send_message(h.info_text(), ephemeral=True)

        @tree.context_menu(name='Deobfuscate file')
        async def deobf_menu(interaction: discord.Interaction, message: discord.Message):
            await h.from_message(interaction, message)

        @tree.error
        async def on_error(interaction: discord.Interaction, error: app_commands.AppCommandError):
            log.error('command error: %r', error, exc_info=error)
            try:
                await InteractionResponder(interaction).refuse(explain('internal'))
            except Exception:
                pass

    async def on_message(self, message):
        if not self.cfg.prefix_commands or message.author.bot:
            return
        args = parse_command(message.content, self.cfg.prefix)
        if args is None:
            return
        try:
            await self.handler.from_chat(message, args)
        except discord.Forbidden:
            log.info('no permission to reply in channel %s', getattr(message.channel, 'id', '?'))
        except Exception:
            log.exception('failure while handling %s%s', self.cfg.prefix, COMMAND)

    async def setup_hook(self):
        if self.cfg.guild_id:
            guild = discord.Object(id=self.cfg.guild_id)
            self.tree.copy_global_to(guild=guild)
            await self.tree.sync(guild=guild)
        else:
            await self.tree.sync()

    async def on_ready(self):
        log.info('ready: %s (id %s), %d servers, chat command: %s', self.user, getattr(self.user, 'id', '?'),
                 len(self.guilds), (self.cfg.prefix + COMMAND) if self.cfg.prefix_commands else 'off')

    async def close(self):
        self.service.close()
        await discord.Client.close(self)


class _NoVoiceWarning(logging.Filter):
    def filter(self, record):
        return 'voice will NOT be supported' not in record.getMessage()


def main(argv=None):
    load_dotenv()
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(levelname)s %(name)s: %(message)s')
    logging.getLogger('discord.client').addFilter(_NoVoiceWarning())
    try:
        cfg = Config()
    except ValueError as e:
        print('Invalid configuration: %s' % e, file=sys.stderr)
        return 1
    if not cfg.token:
        print('DISCORD_TOKEN is not set. Copy .env.example to .env and fill in the token.', file=sys.stderr)
        return 1
    bad = [(n, m) for n, ok, m in check_tools() if not ok]
    if bad:
        for n, m in bad:
            print('Problem with a Luau tool: %s: %s' % (n, m), file=sys.stderr)
        print('See the "Luau binaries" section of the README.', file=sys.stderr)
        return 1
    client = DeobfClient(cfg)
    try:
        client.run(cfg.token, log_handler=None)
    except discord.LoginFailure:
        print('Discord rejected the token. Check DISCORD_TOKEN in .env.', file=sys.stderr)
        return 1
    except discord.PrivilegedIntentsRequired:
        print('The %s%s command needs the "Message Content Intent". Enable it at discord.com/developers/applications -> '
              'your application -> Bot -> Privileged Gateway Intents -> MESSAGE CONTENT INTENT, then run again.\n'
              'Or set PREFIX_COMMANDS=0 in .env to use slash commands only.' % (cfg.prefix, COMMAND),
              file=sys.stderr)
        return 1
    except discord.HTTPException as e:
        print('Discord rejected the request (HTTP %s): %s' % (e.status, e.text or e.response.reason), file=sys.stderr)
        return 1
    except (OSError, aiohttp.ClientError) as e:
        print("Couldn't connect to Discord (%s). Check your internet connection." % type(e).__name__, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
