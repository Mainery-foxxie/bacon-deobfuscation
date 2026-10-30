"""The live progress card the Discord bot shows while a job runs.

    ⠹ Deobfuscating script
    Working on your sample.

    ✅ Validating URL
    ✅ Downloading content
    **⠹ Detecting obfuscator**
    ▫️ Opening the VM
    ...
    4.2s elapsed

`Card` only holds state and produces plain data (`data()`), so it is easy to test; `embed()` turns that into a
discord.Embed.  The bot redraws it about once a second (see Handler._pump), which moves the spinner and the timer."""
import time

SPINNER = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
RUNNING, DONE, FAILED = 0x5865F2, 0x57F287, 0xED4245
MARK = {'todo': '▫️', 'done': '✅', 'failed': '❌'}

FILE_STEPS = [
    ('detect', 'Detecting obfuscator'),
    ('unpack', 'Opening the VM'),
    ('classify', 'Recognizing handlers'),
    ('decompile', 'Rebuilding the code'),
    ('verify', 'Verifying the result'),
    ('deliver', 'Preparing download'),
]
URL_STEPS = [('validate', 'Validating URL'), ('download', 'Downloading content')] + FILE_STEPS

# engine stage names (see engine.deobfuscate) -> card steps
ENGINE_STAGE = {'analyse': 'detect', 'unpack': 'unpack', 'classify': 'classify', 'decompile': 'decompile',
                'verify': 'verify'}


class Card(object):
    def __init__(self, from_url=False, title='Deobfuscating script', clock=time.monotonic):
        self.steps = list(URL_STEPS if from_url else FILE_STEPS)
        self.state = dict((k, 'todo') for k, _ in self.steps)
        self.title = title
        self.note = None
        self.status = 'running'      # 'running' | 'done' | 'failed'
        self.frame = 0
        self._clock = clock
        self._t0 = clock()
        self._t1 = None

    # ------------------------------------------------------------------ state changes
    def begin(self, key):
        """`key` is the step being worked on now: every step before it is done, every step after it still to do"""
        if self.status != 'running' or key not in self.state:
            return
        seen = False
        for k, _ in self.steps:
            if k == key:
                self.state[k] = 'active'
                seen = True
            else:
                self.state[k] = 'todo' if seen else 'done'
        self.note = None

    def stage(self, name):
        """an engine stage name ('analyse', 'unpack', ...)"""
        key = ENGINE_STAGE.get(name)
        if key is not None:
            self.begin(key)

    def queued(self, position):
        if self.status == 'running':
            self.note = 'Waiting in the queue (position %d).' % position

    def complete(self):
        if self.status == 'running':
            for k in self.state:
                self.state[k] = 'done'
            self.status = 'done'
            self.note = None
            self._t1 = self._clock()

    def fail(self):
        """the step that was running (or the first one still to do) is marked as failed"""
        if self.status != 'running':
            return
        target = next((k for k, _ in self.steps if self.state[k] == 'active'), None) \
            or next((k for k, _ in self.steps if self.state[k] == 'todo'), None)
        if target is not None:
            self.state[target] = 'failed'
        self.status = 'failed'
        self.note = None
        self._t1 = self._clock()

    def tick(self):
        self.frame += 1

    # ------------------------------------------------------------------ what is shown
    def elapsed(self):
        return (self._t1 if self._t1 is not None else self._clock()) - self._t0

    def spinner(self):
        return SPINNER[self.frame % len(SPINNER)]

    def lines(self):
        out = []
        for k, label in self.steps:
            st = self.state[k]
            if st == 'active':
                out.append('**%s %s**' % (self.spinner(), label))
            else:
                out.append('%s %s' % (MARK[st], label))
        return out

    def data(self):
        if self.status == 'done':
            title, color, intro = '✅ Script deobfuscated', DONE, 'All steps finished.'
            footer = 'Finished in %.1fs' % self.elapsed()
        elif self.status == 'failed':
            title, color, intro = '❌ Could not deobfuscate', FAILED, 'The job stopped. The reason is in the message.'
            footer = 'Stopped after %.1fs' % self.elapsed()
        else:
            title, color, intro = '%s %s' % (self.spinner(), self.title), RUNNING, 'Working on your sample.'
            footer = '%.1fs elapsed' % self.elapsed()
        if self.note:
            intro += '\n' + self.note
        return dict(title=title, description=intro + '\n\n' + '\n'.join(self.lines()), color=color, footer=footer)

    def embed(self):
        import discord
        d = self.data()
        e = discord.Embed(title=d['title'], description=d['description'], color=d['color'])
        e.set_footer(text=d['footer'])
        return e
