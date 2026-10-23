"""Drive the real discord.py command machinery with a fake network: build interaction payloads, feed them to the
client's CommandTree, and record every HTTP call the bot makes.  (Nothing here talks to Discord.)"""
import json

import discord
from multidict import CIMultiDict

APP_ID = 4242


class FakeResponse(object):
    def __init__(self, status=200, body=None, ratelimit=False):
        self.status = status
        self.reason = 'OK' if status < 300 else 'ERR'
        self.headers = CIMultiDict({'Content-Type': 'application/json'} if body is not None else {})
        if ratelimit:
            self.headers.update({'X-Ratelimit-Bucket': 'fake', 'X-Ratelimit-Limit': '5', 'X-Ratelimit-Remaining': '4',
                                 'X-Ratelimit-Reset-After': '0.05'})
        self._body = json.dumps(body) if body is not None else ''

    async def text(self, encoding='utf-8'):
        return self._body

    async def json(self):
        return json.loads(self._body)

    async def read(self):
        return self._body.encode()


class _Ctx(object):
    def __init__(self, resp):
        self.resp = resp

    async def __aenter__(self):
        return self.resp

    async def __aexit__(self, *a):
        return False


def message_payload(content='', attachments=(), mid=8001, channel_id=777, author_id=None, reference=None):
    return {'id': str(mid), 'channel_id': str(channel_id), 'content': content, 'attachments': list(attachments), 'embeds': [],
            'pinned': False, 'mention_everyone': False, 'tts': False, 'type': 20, 'flags': 64,
            'author': {'id': str(author_id or APP_ID), 'username': 'deobfbot', 'discriminator': '0', 'avatar': None,
                       'bot': author_id is None},
            'edited_timestamp': None, 'timestamp': '2026-10-21T00:00:00+00:00', 'mentions': [], 'mention_roles': []}


class FakeSession(object):
    """stands in for aiohttp.ClientSession; records calls as dicts"""

    def __init__(self):
        self.calls = []
        self.messages = {}
        self._next_id = 9000
        self.forbid_files = False
        self.reject_reference = False
        self.forbid_all = False

    def _body(self, data):
        rec = {'json': None, 'files': [], 'form_json': None}
        if isinstance(data, (bytes, bytearray, str)):
            rec['json'] = json.loads(data.decode('utf-8') if not isinstance(data, str) else data)
        elif data is not None and hasattr(data, '_fields'):
            for opts, headers, value in data._fields:
                name = opts.get('name')
                if name == 'payload_json':
                    rec['form_json'] = json.loads(value if isinstance(value, str) else value.decode('utf-8'))
                else:
                    body = value.read() if hasattr(value, 'read') else value
                    rec['files'].append({'name': name, 'filename': opts.get('filename'), 'content': body})
                    if hasattr(value, 'seek'):
                        value.seek(0)
        return rec

    def request(self, method, url, **kw):
        rec = dict(method=method, url=url)
        rec.update(self._body(kw.get('data')))
        self.calls.append(rec)
        if '/callback' in url:
            return _Ctx(FakeResponse(200, {'interaction': {'id': '1', 'type': 2, 'response_message_loading': True}}))
        if '/messages/@original' in url:
            return _Ctx(FakeResponse(200, message_payload()))
        if self.forbid_all and '/channels/' in url:
            return _Ctx(FakeResponse(403, {'message': 'Missing Access', 'code': 50001}))
        if self.reject_reference and (rec['json'] or {}).get('message_reference'):
            return _Ctx(FakeResponse(400, {'message': 'Invalid Form Body', 'code': 50035}))
        if self.forbid_files and rec['files']:
            return _Ctx(FakeResponse(403, {'message': 'Missing Permissions', 'code': 50013}))
        tail = url.split('/channels/')[-1] if '/channels/' in url else ''
        parts = tail.split('/')
        if method == 'POST' and len(parts) == 2 and parts[1] == 'messages':
            body = rec['json'] or rec['form_json'] or {}
            self._next_id += 1
            atts = [{'id': str(i), 'filename': f['filename'], 'size': len(f['content']), 'url': 'u', 'proxy_url': 'p'}
                    for i, f in enumerate(rec['files'])]
            msg = message_payload(body.get('content', ''), atts, mid=self._next_id, channel_id=parts[0])
            msg['flags'] = 0
            msg['type'] = 0
            self.messages[str(self._next_id)] = msg
            return _Ctx(FakeResponse(200, msg, ratelimit=True))
        if method == 'PATCH' and len(parts) == 3 and parts[1] == 'messages':
            body = rec['json'] or rec['form_json'] or {}
            msg = dict(self.messages.get(parts[2]) or message_payload(mid=parts[2], channel_id=parts[0]))
            if 'content' in body:
                msg['content'] = body['content']
            if rec['files']:
                msg['attachments'] = [{'id': str(i), 'filename': f['filename'], 'size': len(f['content']), 'url': 'u',
                                       'proxy_url': 'p'} for i, f in enumerate(rec['files'])]
            self.messages[parts[2]] = msg
            return _Ctx(FakeResponse(200, msg, ratelimit=True))
        if method == 'GET' and len(parts) == 3 and parts[1] == 'messages' and parts[2] in self.messages:
            return _Ctx(FakeResponse(200, self.messages[parts[2]], ratelimit=True))
        return _Ctx(FakeResponse(404, {'message': 'Unknown', 'code': 10008}))

    # helpers
    def callbacks(self):
        return [c for c in self.calls if '/callback' in c['url']]

    def edits(self):
        return [c for c in self.calls if '/messages/@original' in c['url']]

    def sent(self):
        """messages the bot posted to a channel (POST .../messages)"""
        return [c for c in self.calls if c['method'] == 'POST' and c['url'].endswith('/messages')]

    def edited(self):
        return [c for c in self.calls if c['method'] == 'PATCH' and '/messages/' in c['url'] and '@original' not in c['url']]

    def last_text(self):
        """content of the last message the bot put on screen"""
        for c in reversed(self.calls):
            body = c['json'] or c['form_json']
            if not body:
                continue
            if '/callback' in c['url']:
                return (body.get('data') or {}).get('content')
            if 'content' in body:
                return body['content']
        return None

    def delivered(self):
        """every file the bot uploaded to a channel (new message or edit), in order"""
        out = []
        for c in self.calls:
            if c['method'] in ('POST', 'PATCH') and '/channels/' in c['url']:
                out.extend(c['files'])
        return out

    def chat_texts(self):
        """every text the bot wrote in a channel, in order (new messages and edits)"""
        out = []
        for c in self.calls:
            if c['method'] in ('POST', 'PATCH') and '/channels/' in c['url']:
                body = c['json'] or c['form_json'] or {}
                if 'content' in body:
                    out.append(body['content'])
        return out


def attachment(aid, filename, size, url=None):
    return {'id': str(aid), 'filename': filename, 'size': size, 'url': url or 'https://cdn.example/%s/%s' % (aid, filename),
            'proxy_url': 'https://media.example/%s/%s' % (aid, filename), 'content_type': 'text/plain'}


def _base(token, user_id, guild_id=None):
    user = {'id': str(user_id), 'username': 'tester%s' % user_id, 'discriminator': '0', 'global_name': 'Tester', 'avatar': None}
    d = {'id': str(9000 + (abs(hash(token)) % 1000)), 'application_id': str(APP_ID), 'token': token, 'version': 1,
         'locale': 'en-US', 'app_permissions': '0', 'attachment_size_limit': 10 * 1024 * 1024, 'entitlements': [],
         'authorizing_integration_owners': {}}
    if guild_id:
        d['guild_id'] = str(guild_id)
        d['member'] = {'user': user, 'roles': [], 'joined_at': '2026-01-01T00:00:00+00:00', 'permissions': '0', 'flags': 0,
                       'deaf': False, 'mute': False}
    else:
        d['user'] = user
    return d


def slash(token, user_id, name, options, resolved_attachments=None, guild_id=None):
    d = _base(token, user_id, guild_id)
    d['type'] = 2
    d['data'] = {'id': '555', 'name': name, 'type': 1, 'options': options}
    if resolved_attachments:
        d['data']['resolved'] = {'attachments': dict((a['id'], a) for a in resolved_attachments)}
    return d


def context_menu(token, user_id, name, message_id, atts, guild_id=None):
    d = _base(token, user_id, guild_id)
    d['type'] = 2
    msg = {'id': str(message_id), 'channel_id': '777', 'content': 'please check', 'attachments': list(atts), 'embeds': [],
           'pinned': False, 'mention_everyone': False, 'tts': False, 'type': 0, 'flags': 0, 'edited_timestamp': None,
           'timestamp': '2026-10-21T00:00:00+00:00', 'mentions': [], 'mention_roles': [],
           'author': {'id': '99', 'username': 'someone', 'discriminator': '0', 'avatar': None}}
    d['data'] = {'id': '556', 'name': name, 'type': 3, 'target_id': str(message_id),
                 'resolved': {'messages': {str(message_id): msg}}}
    return d


async def invoke(client, payload, session=None, cdn=None):
    """run one interaction through the client's command tree; returns the FakeSession"""
    session = session or FakeSession()
    cdn = cdn or {}

    async def get_from_cdn(url):
        v = cdn[url]
        if isinstance(v, Exception):
            raise v
        return v

    client.http.get_from_cdn = get_from_cdn
    if client.user is None:
        client._connection.user = discord.ClientUser(state=client._connection, data={
            'id': str(APP_ID), 'username': 'deobfbot', 'discriminator': '0', 'avatar': None, 'bot': True, 'global_name': None})
    interaction = discord.Interaction(data=payload, state=client._connection)
    interaction._session = session
    await client.tree._call(interaction)
    return session


def ensure_bot_user(client):
    if client.user is None:
        client._connection.user = discord.ClientUser(state=client._connection, data={
            'id': str(APP_ID), 'username': 'deobfbot', 'discriminator': '0', 'avatar': None, 'bot': True, 'global_name': None})


def prepare_http(client, session, cdn=None):
    """route every request the library makes (REST and webhook) to the fake session"""
    import asyncio
    client.http._HTTPClient__session = session
    client.http.token = 'fake-token'
    ev = asyncio.Event()
    ev.set()
    client.http._global_over = ev
    cdn = cdn or {}

    async def get_from_cdn(url):
        v = cdn[url]
        if isinstance(v, Exception):
            raise v
        return v

    client.http.get_from_cdn = get_from_cdn


def user_payload(user_id, bot=False):
    d = {'id': str(user_id), 'username': 'tester%s' % user_id, 'discriminator': '0', 'global_name': 'Tester', 'avatar': None}
    if bot:
        d['bot'] = True
    return d


def chat_payload(content, user_id=123, atts=(), mid=5001, channel_id=777, reference=None, guild_id=None, bot=False):
    d = {'id': str(mid), 'channel_id': str(channel_id), 'content': content, 'attachments': list(atts), 'embeds': [],
         'pinned': False, 'mention_everyone': False, 'tts': False, 'type': 19 if reference else 0, 'flags': 0,
         'edited_timestamp': None, 'timestamp': '2026-10-21T00:00:00+00:00', 'mentions': [], 'mention_roles': [],
         'author': user_payload(user_id, bot)}
    if guild_id:
        d['guild_id'] = str(guild_id)
    if reference:
        d['message_reference'] = {'message_id': str(reference['id']), 'channel_id': str(channel_id)}
        if reference.get('embed_resolved', True):
            d['referenced_message'] = chat_payload(reference.get('content', ''), user_id=reference.get('author', 99),
                                                   atts=reference.get('atts', ()), mid=reference['id'], channel_id=channel_id)
    return d


def dm_channel(client, user_id=123, channel_id=777):
    return discord.DMChannel(me=client.user, state=client._connection,
                             data={'id': str(channel_id), 'type': 1, 'recipients': [user_payload(user_id)]})


def guild_channel(client, guild_id=900, channel_id=777, perms=101376):
    """a real Guild + TextChannel in the client's cache; the bot's own permissions are `perms` (everyone role)"""
    ensure_bot_user(client)
    st = client._connection
    gd = {'id': str(guild_id), 'name': 'Test Guild', 'roles': [{'id': str(guild_id), 'name': '@everyone', 'permissions': str(perms),
          'position': 0, 'color': 0, 'hoist': False, 'managed': False, 'mentionable': False, 'flags': 0}],
          'channels': [], 'members': [], 'emojis': [], 'stickers': [], 'features': [], 'afk_timeout': 300,
          'verification_level': 0, 'default_message_notifications': 0, 'explicit_content_filter': 0, 'mfa_level': 0,
          'premium_tier': 0, 'preferred_locale': 'en-US', 'system_channel_flags': 0, 'nsfw_level': 0,
          'owner_id': '1', 'afk_channel_id': None, 'system_channel_id': None, 'premium_progress_bar_enabled': False,
          'member_count': 2, 'large': False, 'unavailable': False}
    guild = discord.Guild(data=gd, state=st)
    st._add_guild(guild)
    guild._add_member(discord.Member._from_client_user(user=client.user, guild=guild, state=st))
    ch = discord.TextChannel(state=st, guild=guild, data={'id': str(channel_id), 'type': 0, 'name': 'general', 'position': 0,
                                                          'permission_overwrites': [], 'nsfw': False, 'topic': None,
                                                          'parent_id': None, 'rate_limit_per_user': 0})
    guild._add_channel(ch)
    return ch


async def say(client, content, atts=(), user_id=123, cdn=None, session=None, reference=None, mid=5001, channel=None, bot=False):
    """deliver one chat message to the client's on_message; returns the FakeSession"""
    session = session or FakeSession()
    ensure_bot_user(client)
    prepare_http(client, session, cdn)
    channel = channel or dm_channel(client, user_id)
    gid = getattr(getattr(channel, 'guild', None), 'id', None)
    payload = chat_payload(content, user_id, atts, mid=mid, channel_id=channel.id, reference=reference, guild_id=gid, bot=bot)
    if reference and not reference.get('embed_resolved', True):
        session.messages[str(reference['id'])] = chat_payload(reference.get('content', ''), user_id=reference.get('author', 99),
                                                              atts=reference.get('atts', ()), mid=reference['id'],
                                                              channel_id=channel.id)
    msg = discord.Message(state=client._connection, channel=channel, data=payload)
    await client.on_message(msg)
    return session
