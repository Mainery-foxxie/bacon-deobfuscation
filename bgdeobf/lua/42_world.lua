-- part 4b: the pretend world: the DataModel (`game`), the services, the local player and its character, the camera.
-- Everything here is made up, once, when the runtime starts; the log only shows what the script touches.
do
    local newinst, defm, ref, attach, mark_child = rt.newinst, rt.defm, rt.ref, rt.attach, rt.mark_child
    local DEF, SUPER = rt.DEF, rt.SUPER
    local V3, V2, CF = rt.V3, rt.V2, rt.CF
    local E = rt.enum_item
    local emit, repr, quote, newvar = rt.emit, rt.repr, rt.quote, rt.newvar
    local fire, fire_if, getsig, setprop = rt.fire, rt.fire_if, rt.getsig, rt.setprop

    local PLACE_ID = CONFIG.place_id or 0
    local GAME_ID = CONFIG.game_id or PLACE_ID
    local JOB_ID = CONFIG.job_id or '8c1f5d2e-3b7a-4d96-a0e4-5f6b7c8d9e01'
    local UID = CONFIG.user_id or 2100000001
    local UNAME = CONFIG.username or 'Player1'
    DEF.Player.UserId = UID
    DEF.Player.CharacterAppearanceId = UID
    DEF.Workspace.DistributedGameTime = nil
    rt.PLACE_ID, rt.USER_ID, rt.USERNAME = PLACE_ID, UID, UNAME

    -- a thing the script asked for that depends on how it is run (the place it believes it is in, who it is): written once
    local probed = {}
    function rt.probe(name, value)
        if probed[name] then return end
        probed[name] = true
        emit('probe', name .. ' -> ' .. repr(value, 1))
    end

    -- ------------------------------------------------------------------------------------------ the DataModel
    local SERVICES = {}
    for w in string.gmatch([[Workspace Players Lighting ReplicatedStorage ReplicatedFirst ServerStorage ServerScriptService
        StarterGui StarterPack StarterPlayer Teams SoundService Chat TextChatService LocalizationService TestService RunService
        TweenService UserInputService ContextActionService GuiService HttpService TeleportService MarketplaceService
        CollectionService Debris PathfindingService PhysicsService ProximityPromptService VirtualUser VirtualInputManager CoreGui
        Stats LogService ScriptContext NetworkClient SocialService BadgeService GroupService PolicyService AssetService
        InsertService TextService AvatarEditorService AnalyticsService MemStorageService HapticService VoiceChatService
        NotificationService ContentProvider KeyframeSequenceProvider UserService MessagingService DataStoreService
        RbxAnalyticsService ExperienceService CorePackages GamePassService ScriptService TouchInputService GamepadService
        LanguageService ThirdPartyUserService Visit VRService BrowserService MouseService JointsService]], '%a+') do
        SERVICES[w] = true
    end
    rt.SERVICES = SERVICES

    local game, gd = newinst('DataModel', {name = 'Game', world = true, var = 'game'})
    gd.how = function() return 'game' end
    rt.game = game

    local services = {}
    local SERVICE_INIT = {}
    rt.SERVICE_INIT = SERVICE_INIT
    local function service(name)
        local s = services[name]
        if s then return s end
        local u, d = newinst(name, {name = name, world = true})
        d.how = function() return 'game:GetService(' .. quote(name) .. ')' end
        d.service = true
        services[name] = u
        attach(game, u)
        local init = SERVICE_INIT[name]
        if init then init(u, d) end
        return u
    end
    rt.service = service

    local IDENT = {PlaceId = PLACE_ID, GameId = GAME_ID, JobId = JOB_ID, PlaceVersion = 1, CreatorId = CONFIG.creator_id or 1,
        PrivateServerId = '', PrivateServerOwnerId = 0, CreatorType = E('CreatorType', 'User')}
    gd.hook_index = function(self, d, key)
        local v = IDENT[key]
        if v ~= nil then
            if key == 'PlaceId' or key == 'GameId' or key == 'JobId' then rt.probe('game.' .. key, v) end
            return v
        end
        if SERVICES[key] then return service(key) end
        if key == 'workspace' then return service('Workspace') end
    end
    gd.psigs = nil

    local function get_service(self, d, name)
        if type(name) ~= 'string' then error('Argument 1 missing or nil', 3) end
        if not SERVICES[name] then error("'" .. name .. "' is not a valid Service name", 3) end
        return service(name)
    end
    defm('DataModel', 'GetService', get_service, true)
    defm('DataModel', 'getService', get_service, true)
    defm('DataModel', 'service', get_service, true)
    defm('DataModel', 'FindService', function(self, d, name) if SERVICES[name] then return service(name) end return nil end, true)
    defm('DataModel', 'IsLoaded', function() return true end, true)
    defm('DataModel', 'BindToClose', function(self, d, fn) rt.note_handler(nil, fn) end)
    defm('DataModel', 'Shutdown', function() end)
    defm('DataModel', 'GetObjects', function(self, d, id)
        emit('net', 'game:GetObjects(' .. repr(id, 1) .. ')', tostring(id), 'GetObjects')
        local m, md = newinst('Model', {name = 'Model', world = true})
        md.how = function() return 'game:GetObjects(' .. repr(id, 1) .. ')[1]' end
        return {m}
    end, true)

    -- ------------------------------------------------------------------------------------------ the network
    local RESP = CONFIG.responses or {}
    local function lookup(url)
        local r = RESP[url]
        if r ~= nil then return r end
        for k, v in next, RESP do
            if ssub(k, -1) == '*' and ssub(url, 1, #k - 1) == ssub(k, 1, -2) then return v end
        end
        return nil
    end
    rt.lookup_response = lookup
    -- one request: logged, and answered with the text given for that url (or nothing)
    function rt.fetch(text, url, method, body)
        emit('net', text, url, method)
        if body ~= nil and type(body) == 'string' and #body > 0 then rt.payload('body', method .. ' ' .. url, body) end
        local r = lookup(url)
        if r == nil then return '' end
        return r
    end

    local function http_get(self, d, url)
        if type(url) ~= 'string' then error('Argument 1 missing or nil', 3) end
        return rt.fetch(ref(self) .. ':HttpGet(' .. quote(url, 300) .. ')', url, 'GET')
    end
    defm('DataModel', 'HttpGet', http_get, true)
    defm('DataModel', 'HttpGetAsync', http_get, true)
    defm('DataModel', 'httpGet', http_get, true)
    local function http_post(self, d, url, data)
        if type(url) ~= 'string' then error('Argument 1 missing or nil', 3) end
        return rt.fetch(ref(self) .. ':HttpPost(' .. quote(url, 300) .. ', ' .. repr(data, 1) .. ')', url, 'POST', type(data) == 'string' and data or nil)
    end
    defm('DataModel', 'HttpPost', http_post, true)
    defm('DataModel', 'HttpPostAsync', http_post, true)

    -- ------------------------------------------------------------------------------------------ the workspace
    SERVICE_INIT.Workspace = function(u, d)
        d.var = 'workspace'
        d.how = function() return 'workspace' end
        local cam, cd = newinst('Camera', {name = 'Camera', world = true})
        mark_child(cam, u, 'index', 'Camera')
        attach(u, cam)
        local terrain = newinst('Terrain', {name = 'Terrain', world = true})
        mark_child(terrain, u, 'index', 'Terrain')
        attach(u, terrain)
        d.props.CurrentCamera = cam
        d.props.Terrain = terrain
        d.hook_index = function(self, dd, key)
            if key == 'DistributedGameTime' then return rt.clock() + 100 end
        end
    end

    local function dpart(inst)
        local d = DATA[inst]
        return d.props.CFrame or DEF.BasePart.CFrame
    end

    defm('WorldRoot', 'Raycast', function() return nil end)
    defm('WorldRoot', 'Blockcast', function() return nil end)
    defm('WorldRoot', 'Spherecast', function() return nil end)
    defm('WorldRoot', 'FindPartOnRay', function(self, d, ray) return nil, V3(0, 0, 0), V3(0, 1, 0) end)
    defm('WorldRoot', 'FindPartOnRayWithIgnoreList', function(self, d, ray) return nil, V3(0, 0, 0), V3(0, 1, 0) end)
    defm('WorldRoot', 'FindPartOnRayWithWhitelist', function(self, d, ray) return nil, V3(0, 0, 0), V3(0, 1, 0) end)
    defm('WorldRoot', 'GetPartBoundsInBox', function() return {} end)
    defm('WorldRoot', 'GetPartBoundsInRadius', function() return {} end)
    defm('WorldRoot', 'GetPartsInPart', function() return {} end)
    defm('WorldRoot', 'GetPartsInRegion3', function() return {} end)
    defm('WorldRoot', 'GetServerTimeNow', function() return rt.epoch() end)
    defm('WorldRoot', 'GetRealPhysicsFPS', function() return 60 end)
    defm('WorldRoot', 'PGSIsEnabled', function() return true end)
    defm('WorldRoot', 'ArePartsTouchingOthers', function() return false end)

    -- ------------------------------------------------------------------------------------------ the camera
    defm('Camera', 'WorldToScreenPoint', function(self, d, v) return V3(960, 540, 10), true end, true)
    defm('Camera', 'WorldToViewportPoint', function(self, d, v) return V3(960, 540, 10), true end, true)
    defm('Camera', 'ViewportPointToRay', function(self, d, x, y) return rt.Ray.new(V3(0, 10, 10), V3(0, 0, -1)) end, true)
    defm('Camera', 'ScreenPointToRay', function(self, d, x, y) return rt.Ray.new(V3(0, 10, 10), V3(0, 0, -1)) end, true)
    defm('Camera', 'GetPartsObscuringTarget', function() return {} end, true)
    defm('Camera', 'GetRenderCFrame', function(self, d) return d.props.CFrame or DEF.Camera.CFrame end, true)
    defm('Camera', 'GetLargestCutoffDistance', function() return 0 end, true)

    -- ------------------------------------------------------------------------------------------ parts and models
    local function isa_part(inst) return rt.isa(DATA[inst].cls, 'BasePart') end
    local function pivot_of(model, d)
        local pp = d.props.PrimaryPart
        if pp ~= nil and KIND[pp] == 'Instance' then return dpart(pp) end
        local kids = d.kids
        for i = 1, #kids do
            if isa_part(kids[i]) then return dpart(kids[i]) end
        end
        return CF(0, 0, 0)
    end

    local function set_cframe(inst, cf)
        setprop(inst, 'CFrame', cf)
    end
    defm('Model', 'GetPivot', function(self, d) return pivot_of(self, d) end, true)
    defm('Model', 'PivotTo', function(self, d, cf)
        local pp = d.props.PrimaryPart
        if pp ~= nil then set_cframe(pp, cf) end
    end)
    defm('Model', 'MoveTo', function(self, d, pos)
        local pp = d.props.PrimaryPart
        if pp ~= nil and KIND[pos] == 'Vector3' then
            local p = DATA[pos]
            set_cframe(pp, CF(p.X, p.Y, p.Z))
        end
    end)
    defm('Model', 'SetPrimaryPartCFrame', function(self, d, cf)
        local pp = d.props.PrimaryPart
        if pp ~= nil then set_cframe(pp, cf) end
    end)
    defm('Model', 'TranslateBy', function() end)
    defm('Model', 'GetBoundingBox', function(self, d) return pivot_of(self, d), V3(4, 5, 2) end)
    defm('Model', 'GetExtentsSize', function() return V3(4, 5, 2) end)
    defm('Model', 'GetScale', function() return 1 end)
    defm('Model', 'ScaleTo', function() end)
    defm('PVInstance', 'GetPivot', function(self, d) return d.props.CFrame or DEF.BasePart.CFrame end, true)
    defm('PVInstance', 'PivotTo', function(self, d, cf) set_cframe(self, cf) end)

    defm('BasePart', 'GetMass', function() return 1 end)
    defm('BasePart', 'GetTouchingParts', function() return {} end)
    defm('BasePart', 'GetConnectedParts', function() return {} end)
    defm('BasePart', 'GetJoints', function() return {} end)
    defm('BasePart', 'GetRootPart', function(self) return self end)
    defm('BasePart', 'GetNetworkOwner', function() return nil end)
    defm('BasePart', 'CanSetNetworkOwnership', function() return true, '' end)
    defm('BasePart', 'IsGrounded', function() return true end)
    defm('BasePart', 'GetVelocityAtPosition', function() return V3(0, 0, 0) end)
    defm('BasePart', 'CanCollideWith', function(self, d, other) return true end)
    defm('BasePart', 'SetNetworkOwner', function() end)
    defm('BasePart', 'SetNetworkOwnershipAuto', function() end)
    defm('BasePart', 'ApplyImpulse', function() end)
    defm('BasePart', 'ApplyAngularImpulse', function() end)
    defm('BasePart', 'BreakJoints', function() end)
    defm('BasePart', 'MakeJoints', function() end)
    defm('BasePart', 'Resize', function() return true end)

    -- ------------------------------------------------------------------------------------------ players
    local players = {}
    rt.players = players
    local mouse_hook = function(self, d, key) if key == 'Target' or key == 'TargetFilter' then return rt.NIL end end

    -- R15 body: name, width, height, depth, x, y (relative to the root part)
    local BODY = {
        {'LowerTorso', 2, 0.4, 1, 0, -0.8}, {'UpperTorso', 2, 1.6, 1, 0, 0.2}, {'Head', 2, 1, 1, 0, 1.5},
        {'LeftUpperArm', 1, 1.2, 1, -1.5, 0.4}, {'LeftLowerArm', 1, 1.1, 1, -1.5, -0.7}, {'LeftHand', 1, 0.3, 1, -1.5, -1.3},
        {'RightUpperArm', 1, 1.2, 1, 1.5, 0.4}, {'RightLowerArm', 1, 1.1, 1, 1.5, -0.7}, {'RightHand', 1, 0.3, 1, 1.5, -1.3},
        {'LeftUpperLeg', 1, 1.2, 1, -0.5, -1.6}, {'LeftLowerLeg', 1, 1.2, 1, -0.5, -2.7}, {'LeftFoot', 1, 0.3, 1, -0.5, -3.4},
        {'RightUpperLeg', 1, 1.2, 1, 0.5, -1.6}, {'RightLowerLeg', 1, 1.2, 1, 0.5, -2.7}, {'RightFoot', 1, 0.3, 1, 0.5, -3.4},
    }

    local function make_character(player, nm, px, py, pz)
        local ws = rt.service('Workspace')
        local ch, cd = newinst('Model', {name = nm, world = true})
        cd.how = function() return ref(player) .. '.Character' end
        cd.inline, cd.depth = true, 1
        attach(ws, ch)
        local function add(cls, pname, props)
            local u, d = newinst(cls, {name = pname, world = true, props = props})
            mark_child(u, ch, 'index', pname)
            attach(ch, u)
            return u, d
        end
        local hrp = add('Part', 'HumanoidRootPart', {Size = V3(2, 2, 1), CFrame = CF(px, py, pz), Transparency = 1, CanCollide = false})
        for i = 1, #BODY do
            local b = BODY[i]
            add('Part', b[1], {Size = V3(b[2], b[3], b[4]), CFrame = CF(px + b[5], py + b[6], pz)})
        end
        local hum, hd = add('Humanoid', 'Humanoid', {DisplayName = nm})
        local animator = newinst('Animator', {name = 'Animator', world = true})
        mark_child(animator, hum, 'index', 'Animator')
        attach(hum, animator)
        add('LocalScript', 'Animate', {})
        cd.props.PrimaryPart = hrp
        hd.hook_set = function(self, d, key, val)
            if key == 'Health' and type(val) == 'number' then
                fire_if(self, 'HealthChanged', val)
                if val <= 0 and not d.dead then
                    d.dead = true
                    fire_if(self, 'Died')
                    if rt.respawn then rt.later(5, function() rt.respawn(player) end) end
                end
            end
        end
        return ch, hum
    end

    local function make_player(nm, uid, is_local, idx, px)
        local psvc = rt.service('Players')
        local pl, pd = newinst('Player', {name = nm, world = true, props = {UserId = uid, DisplayName = nm, AccountAge = 1500}})
        if is_local then
            pd.how = function() return ref(psvc) .. '.LocalPlayer' end
            pd.varname = 'LocalPlayer'
        else
            pd.how = function() return ref(psvc) .. ':GetPlayers()[' .. idx .. ']' end
            pd.varname = nm
        end
        attach(psvc, pl)
        players[#players + 1] = pl
        for _, c in ipairs({{'PlayerGui', 'PlayerGui'}, {'Backpack', 'Backpack'}, {'PlayerScripts', 'Folder'}, {'StarterGear', 'Folder'}}) do
            local kid = newinst(c[2], {name = c[1], world = true})
            mark_child(kid, pl, 'index', c[1])
            attach(pl, kid)
        end
        local ch, hum = make_character(pl, nm, px, 5, 0)
        pd.props.Character = ch
        pd.hook_index = function(self, d, key)
            if key == 'Team' then return rt.NIL end
        end
        if is_local then
            local cam = rt.service('Workspace')
            local cd = DATA[DATA[cam].props.CurrentCamera]
            cd.props.CameraSubject = hum
            cd.props.CFrame = CF(px, 10, 15)
        end
        return pl
    end

    SERVICE_INIT.Players = function(u, d)
        d.strict = true
        d.props.MaxPlayers = 12
        make_player(UNAME, UID, true, 1, 0)
        d.props.LocalPlayer = players[1]
        for i = 1, (CONFIG.other_players or 0) do
            make_player('Player' .. (i + 1), UID + i, false, i + 1, 12 * i)
        end
        d.hook_index = function(self, dd, key)
            if key == 'NumPlayers' then return #players end
        end
    end

    local function copy(t)
        local o = {}
        for i = 1, #t do o[i] = t[i] end
        return o
    end
    defm('Players', 'GetPlayers', function(self, d) return copy(players) end, true)
    defm('Players', 'GetPlayerFromCharacter', function(self, d, ch)
        for i = 1, #players do
            if DATA[players[i]].props.Character == ch then return players[i] end
        end
        return nil
    end, true)
    defm('Players', 'GetPlayerByUserId', function(self, d, id)
        for i = 1, #players do
            if DATA[players[i]].props.UserId == id then return players[i] end
        end
        return nil
    end, true)
    defm('Players', 'GetUserIdFromNameAsync', function(self, d, name)
        for i = 1, #players do
            if DATA[players[i]].name == name then return DATA[players[i]].props.UserId end
        end
        local h = 0
        for i = 1, #tostring(name) do h = (h * 31 + sbyte(tostring(name), i)) % 1000000000 end
        return 100000000 + h
    end, true)
    defm('Players', 'GetNameFromUserIdAsync', function(self, d, id)
        for i = 1, #players do
            if DATA[players[i]].props.UserId == id then return DATA[players[i]].name end
        end
        return 'User' .. tostring(id)
    end, true)
    defm('Players', 'GetCharacterAppearanceInfoAsync', function() return {} end, true)

    function rt.respawn(player)
        local pd = DATA[player]
        if pd.destroyed then return end
        local old = pd.props.Character
        if old ~= nil then
            fire_if(player, 'CharacterRemoving', old)
            rt.destroy(old)
        end
        local px, py, pz = 0, 5, 0
        local ch, hum = make_character(player, pd.name, px, py, pz)
        pd.props.Character = ch
        if players[1] == player then
            local cam = DATA[rt.service('Workspace')].props.CurrentCamera
            DATA[cam].props.CameraSubject = hum
        end
        fire_if(player, 'CharacterAdded', ch)
    end

    defm('Player', 'LoadCharacter', function(self, d) rt.respawn(self) end)
    defm('Player', 'Kick', function(self, d, msg) end)
    defm('Player', 'GetMouse', function(self, d)
        if not d.mouse then
            local m, md = newinst('PlayerMouse', {name = 'Mouse', world = false})
            md.hook_index = mouse_hook
            d.mouse = m
        end
        return d.mouse
    end)
    defm('Player', 'GetRankInGroup', function() return 0 end)
    defm('Player', 'GetRoleInGroup', function() return 'Guest' end)
    defm('Player', 'IsInGroup', function() return false end)
    defm('Player', 'IsFriendsWith', function() return false end)
    defm('Player', 'GetFriendsOnline', function() return {} end)
    defm('Player', 'HasAppearanceLoaded', function() return true end)
    defm('Player', 'IsVerified', function() return false end)
    defm('Player', 'GetNetworkPing', function() return 0.05 end)
    defm('Player', 'GetJoinData', function() return {} end)
    defm('Player', 'DistanceFromCharacter', function(self, d, pos)
        local ch = d.props.Character
        if ch == nil or KIND[pos] ~= 'Vector3' then return 0 end
        local hrp = DATA[ch].props.PrimaryPart
        local cf = DATA[dpart(hrp)]
        local p = DATA[pos]
        return math.sqrt((cf.x - p.X) ^ 2 + (cf.y - p.Y) ^ 2 + (cf.z - p.Z) ^ 2)
    end)

    -- ------------------------------------------------------------------------------------------ the humanoid
    defm('Humanoid', 'MoveTo', function(self, d, pos, part)
        local ch = d.parent
        if ch == nil or KIND[pos] ~= 'Vector3' then return end
        local hrp = DATA[ch].props.PrimaryPart
        if hrp == nil then return end
        local p = DATA[pos]
        -- the walk takes a moment, then the character is there
        rt.later(0.5, function()
            if DATA[hrp].destroyed then return end
            set_cframe(hrp, CF(p.X, p.Y + 3, p.Z))
            fire_if(self, 'MoveToFinished', true)
        end)
    end)
    defm('Humanoid', 'Move', function() end)
    defm('Humanoid', 'ChangeState', function(self, d, st) d.state = st end)
    defm('Humanoid', 'GetState', function(self, d) return d.state or E('HumanoidStateType', 'Running') end)
    defm('Humanoid', 'GetStateEnabled', function() return true end)
    defm('Humanoid', 'SetStateEnabled', function() end)
    defm('Humanoid', 'TakeDamage', function(self, d, n)
        if type(n) ~= 'number' then return end
        local h = d.props.Health or DEF.Humanoid.Health
        local nh = h - n
        if nh < 0 then nh = 0 end
        d.props.Health = nh
        d.hook_set(self, d, 'Health', nh)
    end)
    defm('Humanoid', 'EquipTool', function(self, d, tool)
        if KIND[tool] ~= 'Instance' then return end
        local ch = d.parent
        if ch ~= nil then
            rt.reparent(tool, ch)
            fire_if(tool, 'Equipped', nil)
        end
    end)
    defm('Humanoid', 'UnequipTools', function() end)
    defm('Humanoid', 'GetPlayingAnimationTracks', function() return {} end)
    defm('Humanoid', 'GetAccessories', function() return {} end)
    defm('Humanoid', 'AddAccessory', function() end)
    defm('Humanoid', 'RemoveAccessories', function() end)
    defm('Humanoid', 'ApplyDescription', function() end)
    defm('Humanoid', 'BuildRigFromAttachments', function() end)

    local function load_animation(self, d, anim)
        local tr, td = newinst('AnimationTrack', {name = 'AnimationTrack', world = false, props = {Animation = anim}})
        return tr
    end
    defm('Humanoid', 'LoadAnimation', load_animation)
    defm('Animator', 'LoadAnimation', load_animation)
    defm('Animator', 'GetPlayingAnimationTracks', function() return {} end)
    DEF.AnimationTrack = {Length = 1, IsPlaying = false, Looped = false, Priority = E('AnimationPriority', 'Action'), Speed = 1,
        TimePosition = 0, WeightCurrent = 1, WeightTarget = 1}
    defm('AnimationTrack', 'Play', function(self, d)
        d.props.IsPlaying = true
        local len = d.props.Length or 1
        if not d.props.Looped then
            rt.later(len, function()
                if d.props.IsPlaying then
                    d.props.IsPlaying = false
                    fire_if(self, 'Stopped')
                    fire_if(self, 'Ended')
                end
            end)
        end
    end)
    defm('AnimationTrack', 'Stop', function(self, d)
        if d.props.IsPlaying then
            d.props.IsPlaying = false
            fire_if(self, 'Stopped')
            fire_if(self, 'Ended')
        end
    end)
    defm('AnimationTrack', 'AdjustSpeed', function(self, d, s) d.props.Speed = s end)
    defm('AnimationTrack', 'AdjustWeight', function(self, d, w) d.props.WeightCurrent = w end)
    defm('AnimationTrack', 'GetMarkerReachedSignal', function(self, d, name)
        return rt.newsig(nil, 'MarkerReached', function() return ref(self) .. ':GetMarkerReachedSignal(' .. quote(tostring(name)) .. ')' end)
    end)
    defm('AnimationTrack', 'GetTimeOfKeyframe', function() return 0 end)

    -- tools
    defm('Tool', 'Activate', function(self, d) fire_if(self, 'Activated') end)
    defm('Tool', 'Deactivate', function(self, d) fire_if(self, 'Deactivated') end)
    defm('Sound', 'Play', function(self, d)
        d.props.IsPlaying, d.props.Playing = true, true
        if not d.props.Looped then
            rt.later(d.props.TimeLength or 3, function()
                d.props.IsPlaying, d.props.Playing = false, false
                fire_if(self, 'Ended')
            end)
        end
    end)
    defm('Sound', 'Stop', function(self, d) d.props.IsPlaying, d.props.Playing = false, false end)
    defm('Sound', 'Pause', function(self, d) d.props.IsPlaying, d.props.Playing = false, false end)
    defm('Sound', 'Resume', function(self, d) d.props.IsPlaying, d.props.Playing = true, true end)
    DEF.Sound.TimeLength = 3

    -- the character is `Players.LocalPlayer.Character`: make everything exist once at the start
    rt.service('Workspace')
    rt.service('Players')
end
