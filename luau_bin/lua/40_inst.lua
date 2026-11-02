-- part 4: Instances.  An Instance is a userdata whose state (class, name, properties, children, parent, signals) lives in
-- DATA.  Instances of the pretend world (game, Players, the character...) are "world" instances: a child that does not
-- exist yet is made up the first time the script asks for it, so `workspace.Map.Door` and
-- `ReplicatedStorage:WaitForChild("Remotes")` just work.  Instances the script makes itself (Instance.new) are honest:
-- they only have what the script gave them.
do
    local newobj, defkind = rt.newobj, rt.defkind
    local V3, V2, CF, C3, UD2, UD = rt.V3, rt.V2, rt.CF, rt.C3, rt.UD2, rt.UD
    local E = rt.enum_item
    local emit, repr, quote, newvar = rt.emit, rt.repr, rt.quote, rt.newvar

    -- ------------------------------------------------------------------------------------------ the class tree
    local SUPER = {}
    for c, p in string.gmatch([[
        Model:PVInstance Actor:Model WorldRoot:Model Workspace:WorldRoot PVInstance:Instance
        BasePart:PVInstance Part:BasePart WedgePart:BasePart CornerWedgePart:BasePart TrussPart:BasePart Seat:Part
        VehicleSeat:BasePart SpawnLocation:Part TriangleMeshPart:BasePart MeshPart:TriangleMeshPart PartOperation:TriangleMeshPart
        UnionOperation:PartOperation NegateOperation:PartOperation Terrain:BasePart
        Tool:BackpackItem BackpackItem:Instance HopperBin:BackpackItem Accessory:Accoutrement Accoutrement:Instance Hat:Accoutrement
        GuiBase:Instance GuiBase2d:GuiBase GuiObject:GuiBase2d Frame:GuiObject ScrollingFrame:GuiObject TextLabel:GuiObject
        GuiButton:GuiObject TextButton:GuiButton ImageButton:GuiButton TextBox:GuiObject ImageLabel:GuiObject
        ViewportFrame:GuiObject CanvasGroup:GuiObject VideoFrame:GuiObject LayerCollector:GuiBase2d ScreenGui:LayerCollector
        BillboardGui:LayerCollector SurfaceGui:LayerCollector BasePlayerGui:Instance PlayerGui:BasePlayerGui CoreGui:BasePlayerGui
        StarterGui:BasePlayerGui UIBase:Instance UIComponent:UIBase UIConstraint:UIComponent UILayout:UIComponent
        UIGridStyleLayout:UILayout UIListLayout:UIGridStyleLayout UIGridLayout:UIGridStyleLayout UIPageLayout:UIGridStyleLayout
        UITableLayout:UIGridStyleLayout UICorner:UIComponent UIStroke:UIComponent UIPadding:UIComponent UIScale:UIComponent
        UIGradient:UIComponent UIAspectRatioConstraint:UIConstraint UISizeConstraint:UIConstraint UITextSizeConstraint:UIConstraint
        LuaSourceContainer:Instance BaseScript:LuaSourceContainer Script:BaseScript LocalScript:Script ModuleScript:LuaSourceContainer
        BaseRemoteEvent:Instance RemoteEvent:BaseRemoteEvent UnreliableRemoteEvent:BaseRemoteEvent
        ValueBase:Instance BoolValue:ValueBase IntValue:ValueBase NumberValue:ValueBase StringValue:ValueBase ObjectValue:ValueBase
        Color3Value:ValueBase Vector3Value:ValueBase CFrameValue:ValueBase BrickColorValue:ValueBase RayValue:ValueBase
        JointInstance:Instance Weld:JointInstance Motor6D:JointInstance Constraint:Instance
        BodyMover:Instance BodyVelocity:BodyMover BodyPosition:BodyMover BodyGyro:BodyMover BodyForce:BodyMover BodyAngularVelocity:BodyMover
        Light:Instance PointLight:Light SpotLight:Light SurfaceLight:Light PostEffect:Instance BloomEffect:PostEffect BlurEffect:PostEffect
        ColorCorrectionEffect:PostEffect SunRaysEffect:PostEffect DepthOfFieldEffect:PostEffect FaceInstance:Instance Decal:FaceInstance
        Texture:Decal TweenBase:Instance Tween:TweenBase Mouse:Instance PlayerMouse:Mouse
    ]], '(%w+):(%w+)') do SUPER[c] = p end
    rt.SUPER = SUPER

    local function isa(cls, target)
        local c = cls
        while c do
            if c == target then return true end
            c = SUPER[c]
        end
        return target == 'Instance'
    end
    rt.isa = isa

    -- ------------------------------------------------------------------------------------------ default properties
    local BCnew = rt.BrickColor.new
    local white, black, grey = C3(1, 1, 1), C3(0, 0, 0), C3(0.63, 0.63, 0.63)
    local ZERO3 = V3(0, 0, 0)
    local DEF = {
        Instance = {Archivable = true},
        BasePart = {Anchored = false, CanCollide = true, CanTouch = true, CanQuery = true, Transparency = 0, Reflectance = 0, Locked = false,
            Size = V3(4, 1, 2), CFrame = CF(0, 0, 0), Material = E('Material', 'Plastic'), Color = grey, BrickColor = BCnew('Medium stone grey'),
            Massless = false, CastShadow = true, Velocity = ZERO3, AssemblyLinearVelocity = ZERO3, AssemblyAngularVelocity = ZERO3,
            RotVelocity = ZERO3, Mass = 1, AssemblyMass = 1, CollisionGroup = 'Default', Orientation = ZERO3},
        Humanoid = {Health = 100, MaxHealth = 100, WalkSpeed = 16, JumpPower = 50, JumpHeight = 7.2, HipHeight = 2, UseJumpPower = true,
            Sit = false, PlatformStand = false, AutoRotate = true, DisplayName = '', RigType = E('RigType', 'R15'), MoveDirection = ZERO3,
            FloorMaterial = E('Material', 'Plastic'), Jump = false, NameDisplayDistance = 100, HealthDisplayDistance = 100,
            RequiresNeck = true, BreakJointsOnDeath = true, WalkToPoint = ZERO3},
        GuiObject = {Visible = true, Active = false, Size = UD2(0, 100, 0, 100), Position = UD2(0, 0, 0, 0), AnchorPoint = V2(0, 0),
            BackgroundColor3 = grey, BackgroundTransparency = 0, BorderSizePixel = 1, BorderColor3 = C3(0.1, 0.1, 0.1), ZIndex = 1,
            LayoutOrder = 0, Rotation = 0, ClipsDescendants = false, AbsoluteRotation = 0, Selectable = false, Interactable = true,
            SizeConstraint = E('SizeConstraint', 'RelativeXY'), AutomaticSize = E('AutomaticSize', 'None'), BorderMode = E('BorderMode', 'Outline')},
        TextLabel = {Text = 'Label', TextColor3 = C3(0.1, 0.1, 0.1), TextSize = 14, TextScaled = false, TextWrapped = false,
            Font = E('Font', 'SourceSans'), TextXAlignment = E('TextXAlignment', 'Center'), TextYAlignment = E('TextYAlignment', 'Center'),
            TextTransparency = 0, TextStrokeTransparency = 1, TextStrokeColor3 = black, RichText = false, TextBounds = V2(0, 0),
            TextTruncate = E('TextTruncate', 'None'), LineHeight = 1, MaxVisibleGraphemes = -1, TextDirection = E('TextDirection', 'Auto')},
        TextButton = {Text = 'Button', TextColor3 = C3(0.1, 0.1, 0.1), TextSize = 14, TextScaled = false, TextWrapped = false,
            Font = E('Font', 'SourceSans'), TextXAlignment = E('TextXAlignment', 'Center'), TextYAlignment = E('TextYAlignment', 'Center'),
            TextTransparency = 0, TextStrokeTransparency = 1, TextStrokeColor3 = black, RichText = false, TextBounds = V2(0, 0),
            AutoButtonColor = true, Modal = false, Style = E('ButtonStyle', 'Custom')},
        TextBox = {Text = '', PlaceholderText = '', PlaceholderColor3 = C3(0.7, 0.7, 0.7), TextColor3 = C3(0.1, 0.1, 0.1), TextSize = 14,
            TextScaled = false, TextWrapped = false, Font = E('Font', 'SourceSans'), TextXAlignment = E('TextXAlignment', 'Center'),
            TextYAlignment = E('TextYAlignment', 'Center'), ClearTextOnFocus = true, MultiLine = false, TextEditable = true,
            ShowNativeInput = true, CursorPosition = 1, TextBounds = V2(0, 0), RichText = false},
        ImageLabel = {Image = '', ImageColor3 = white, ImageTransparency = 0, ScaleType = E('ScaleType', 'Stretch'),
            ImageRectOffset = V2(0, 0), ImageRectSize = V2(0, 0), ResampleMode = E('ResamplerMode', 'Default')},
        ImageButton = {Image = '', ImageColor3 = white, ImageTransparency = 0, ScaleType = E('ScaleType', 'Stretch'),
            ImageRectOffset = V2(0, 0), ImageRectSize = V2(0, 0), AutoButtonColor = true, HoverImage = '', PressedImage = ''},
        ScrollingFrame = {CanvasSize = UD2(0, 0, 2, 0), CanvasPosition = V2(0, 0), ScrollBarThickness = 12, ScrollingEnabled = true,
            AbsoluteCanvasSize = V2(0, 0), AbsoluteWindowSize = V2(0, 0), AutomaticCanvasSize = E('AutomaticSize', 'None'),
            ScrollingDirection = E('ScrollingDirection', 'XY'), ScrollBarImageColor3 = C3(0, 0, 0), ScrollBarImageTransparency = 0,
            ElasticBehavior = E('ElasticBehavior', 'WhenScrollable')},
        ScreenGui = {ResetOnSpawn = true, Enabled = true, DisplayOrder = 0, IgnoreGuiInset = false, ZIndexBehavior = E('ZIndexBehavior', 'Sibling')},
        BillboardGui = {Enabled = true, AlwaysOnTop = false, Size = UD2(0, 100, 0, 100), StudsOffset = ZERO3, MaxDistance = 0, LightInfluence = 1},
        UIListLayout = {AbsoluteContentSize = V2(0, 0), Padding = UD(0, 0), FillDirection = E('FillDirection', 'Vertical'),
            SortOrder = E('SortOrder', 'LayoutOrder'), HorizontalAlignment = E('HorizontalAlignment', 'Left'),
            VerticalAlignment = E('VerticalAlignment', 'Top')},
        UIGridLayout = {AbsoluteContentSize = V2(0, 0), CellSize = UD2(0, 100, 0, 100), CellPadding = UD2(0, 5, 0, 5),
            SortOrder = E('SortOrder', 'LayoutOrder')},
        UICorner = {CornerRadius = UD(0, 8)},
        UIStroke = {Thickness = 1, Color = black, Transparency = 0, Enabled = true, ApplyStrokeMode = E('ApplyStrokeMode', 'Contextual'),
            LineJoinMode = E('LineJoinMode', 'Round')},
        UIPadding = {PaddingLeft = UD(0, 0), PaddingRight = UD(0, 0), PaddingTop = UD(0, 0), PaddingBottom = UD(0, 0)},
        UIScale = {Scale = 1},
        Sound = {Volume = 0.5, Playing = false, IsPlaying = false, IsLoaded = true, TimePosition = 0, TimeLength = 0, Looped = false, PlaybackSpeed = 1, SoundId = ''},
        BoolValue = {Value = false}, IntValue = {Value = 0}, NumberValue = {Value = 0}, StringValue = {Value = ''},
        Camera = {CFrame = CF(0, 10, 10), Focus = CF(0, 0, 0), FieldOfView = 70, ViewportSize = V2(1920, 1080),
            CameraType = E('CameraType', 'Custom'), NearPlaneZ = -0.1, MaxAxisFieldOfView = 70},
        Workspace = {Gravity = 196.2, FilteringEnabled = true, StreamingEnabled = false, DistributedGameTime = 0, AllowThirdPartySales = false},
        Lighting = {Brightness = 2, ClockTime = 14, TimeOfDay = '14:00:00', FogEnd = 100000, FogStart = 0, FogColor = C3(0.75, 0.75, 0.75),
            Ambient = C3(0.5, 0.5, 0.5), OutdoorAmbient = C3(0.5, 0.5, 0.5), GlobalShadows = true, ExposureCompensation = 0,
            GeographicLatitude = 41.7, ColorShift_Top = black, ColorShift_Bottom = black},
        ClickDetector = {MaxActivationDistance = 32},
        ProximityPrompt = {ActionText = 'Interact', ObjectText = '', HoldDuration = 0, MaxActivationDistance = 10, Enabled = true,
            KeyboardKeyCode = E('KeyCode', 'E'), RequiresLineOfSight = true, ClickablePrompt = true},
        Tool = {CanBeDropped = true, RequiresHandle = true, Enabled = true, ToolTip = '', Grip = CF(0, 0, 0)},
        Highlight = {Enabled = true, FillColor = C3(1, 0, 0), OutlineColor = white, FillTransparency = 0.5, OutlineTransparency = 0},
        ParticleEmitter = {Enabled = true, Rate = 20},
        Fire = {Enabled = true, Heat = 9, Size = 5}, Smoke = {Enabled = true}, Sparkles = {Enabled = true},
        PointLight = {Brightness = 1, Range = 8, Enabled = true, Color = white},
        Mouse = {X = 960, Y = 540, ViewSizeX = 1920, ViewSizeY = 1080, Hit = CF(0, 0, 0), Origin = CF(0, 0, 0)},
        Player = {UserId = CONFIG.user_id or 1, AccountAge = 365, MembershipType = E('MembershipType', 'None'), Neutral = true, CameraMode = E('CameraMode', 'Classic'),
            TeamColor = BCnew('White'), RespawnTime = 5, FollowUserId = 0, CharacterAppearanceId = CONFIG.user_id or 1, CanLoadCharacterAppearance = true},
    }
    rt.DEF = DEF
    local function default(cls, key)
        local c = cls
        while c do
            local t = DEF[c]
            if t then
                local v = t[key]
                if v ~= nil then return v end
            end
            c = SUPER[c]
        end
        return DEF.Instance[key]
    end
    rt.default = default

    -- names that are certainly a property and not a child: a world instance answers these with a Ghost, not a made-up child
    local PROPLIKE = {}
    for w in string.gmatch([[Value Text Visible Enabled Size Position CFrame Color Color3 Transparency Health MaxHealth Anchored CanCollide
        Material WalkSpeed JumpPower Image TextColor3 BackgroundColor3 Volume Playing Looped Brightness Range Duration Title Description
        Rotation Orientation Velocity Mass Locked Archivable Scale Offset Thickness Padding Font TextSize Active Selectable ZIndex LayoutOrder
        PrimaryPart Adornee Target Hit Origin X Y Z UserId DisplayName AccountAge TeamColor Team Neutral Character Humanoid
        CurrentCamera FieldOfView ViewportSize Gravity ClockTime TimeOfDay FogEnd Ambient]], '%a[%w]*') do PROPLIKE[w] = true end
    PROPLIKE.Humanoid = nil          -- `Character.Humanoid` is a child
    PROPLIKE.Character = nil
    PROPLIKE.CurrentCamera = nil     -- a child of Workspace (made at start-up)

    -- ------------------------------------------------------------------------------------------ names in the log
    local function dot(k)
        if type(k) == 'string' and string.find(k, '^[%a_][%w_]*$') and not rt.RESERVED[k] then return '.' .. k end
        return '[' .. repr(k, 2) .. ']'
    end
    rt.dot = dot

    local function ref(inst)
        local d = DATA[inst]
        if d.var then return d.var end
        if d.inline and d.how and d.depth <= 4 then return d.how() end
        local expr = d.how and d.how() or ('nil --[[' .. d.cls .. ']]')
        local var = newvar(d.varname or d.name)
        d.var = var
        emit('def', 'local ' .. var .. ' = ' .. expr)
        return var
    end
    rt.ref = ref
    rt.REPR.Instance = ref

    local function fullname(inst)
        local parts = {}
        local c, guard = inst, 0
        while c ~= nil and guard < 60 do
            local d = DATA[c]
            if d.cls == 'DataModel' then break end
            parts[#parts + 1] = d.name
            c = d.parent
            guard = guard + 1
        end
        local out = {}
        for i = #parts, 1, -1 do out[#out + 1] = parts[i] end
        return tconcat(out, '.')
    end
    rt.fullname = fullname

    -- how a child that the script has not met yet is written in the log
    local function how_child(parent, style, name)
        if style == 'index' then return function() return ref(parent) .. dot(name) end end
        return function() return ref(parent) .. ':' .. style .. '(' .. quote(name, 80) .. ')' end
    end
    rt.how_child = how_child
    -- the child is written as a path from its parent (`workspace.Map.Door`) as long as the path stays short
    local function mark_child(child, parent, style, name)
        local cd, pd = DATA[child], DATA[parent]
        cd.how = how_child(parent, style, name)
        cd.inline = true
        cd.depth = (pd.inline and pd.depth or 0) + 1
    end
    rt.mark_child = mark_child

    -- ------------------------------------------------------------------------------------------ making instances
    local idn = 0
    local function newinst(cls, init)
        idn = idn + 1
        local d = init or {}
        d.cls = cls
        d.name = d.name or cls
        d.props = d.props or {}
        d.kids = {}
        d.id = idn
        local u = newobj('Instance', d)
        return u, d
    end
    rt.newinst = newinst

    -- ------------------------------------------------------------------------------------------ signals
    local SIGNALS = {}
    for w in string.gmatch([[Changed ChildAdded ChildRemoved DescendantAdded DescendantRemoving AncestryChanged Destroying AttributeChanged
        Touched TouchEnded MouseButton1Click MouseButton1Down MouseButton1Up MouseButton2Click MouseButton2Down MouseButton2Up MouseEnter
        MouseLeave MouseMoved MouseWheelForward MouseWheelBackward Activated Deactivated InputBegan InputChanged InputEnded FocusLost
        Focused ReturnPressedFromOnScreenKeyboard SelectionGained SelectionLost Heartbeat RenderStepped Stepped PreRender PreAnimation
        PreSimulation PostSimulation Died HealthChanged Running Jumping Climbing Swimming Seated FreeFalling FallingDown GettingUp
        PlatformStanding StateChanged MoveToFinished CharacterAdded CharacterRemoving CharacterAppearanceLoaded PlayerAdded PlayerRemoving
        OnClientEvent OnServerEvent Event Completed PlaybackStateChanged Triggered TriggerEnded PromptShown PromptHidden
        PromptButtonHoldBegan PromptButtonHoldEnded Equipped Unequipped Loaded Ended Played Paused Resumed Stopped DidLoop KeyframeReached
        Button1Down Button1Up Button2Down Button2Up Idle Move KeyDown KeyUp WheelForward WheelBackward Chatted Idled
        LocalPlayerArrivedFromTeleport TeleportInitFailed PromptPurchaseFinished PromptGamePassPurchaseFinished Error MessageOut
        UserInputTypeChanged LastInputTypeChanged TextBoxFocused TextBoxFocusReleased WindowFocused WindowFocusReleased JumpRequest
        TouchTap TouchPan TouchPinch TouchLongPress TouchSwipe TouchStarted TouchMoved TouchEnded Selected Deselected
        ItemChanged ChildRemoving]], '%a[%w]*') do SIGNALS[w] = true end
    rt.SIGNALS = SIGNALS
    local FRAMESIG = {Heartbeat = true, RenderStepped = true, Stepped = true, PreRender = true, PreAnimation = true,
        PreSimulation = true, PostSimulation = true}

    local frame_sigs = {}
    local function newsig(owner, name, expr)
        local sd = {owner = owner, name = name, conns = {}, waiters = {}, expr = expr}
        local s = newobj('RBXScriptSignal', sd)
        if owner ~= nil and FRAMESIG[name] then
            sd.frame = true
            frame_sigs[#frame_sigs + 1] = s
        end
        return s
    end
    rt.newsig = newsig
    rt.frame_sigs = frame_sigs

    local function getsig(self, d, key)
        local sigs = d.sigs
        if sigs == nil then sigs = {}; d.sigs = sigs end
        local s = sigs[key]
        if s == nil then
            s = newsig(self, key, function() return ref(self) .. dot(key) end)
            sigs[key] = s
        end
        return s
    end
    rt.getsig = getsig

    local function fire(sig, ...)
        local sd = DATA[sig]
        local conns = sd.conns
        local n = #conns
        if n > 0 then
            local copy = {}
            for i = 1, n do copy[i] = conns[i] end
            for i = 1, n do
                local cd = DATA[copy[i]]
                if cd.on then
                    if cd.once then rt.disconnect(copy[i]) end
                    rt.spawn(cd.fn, ...)
                end
            end
        end
        local w = sd.waiters
        if #w > 0 then
            sd.waiters = {}
            for i = 1, #w do rt.wake(w[i], ...) end
        end
    end
    rt.fire = fire
    -- fire a signal of an instance only when the script has looked at it
    local function fire_if(inst, name, ...)
        local sigs = DATA[inst].sigs
        local s = sigs and sigs[name]
        if s then fire(s, ...) end
    end
    rt.fire_if = fire_if

    function rt.disconnect(conn)
        local cd = DATA[conn]
        if not cd.on then return end
        cd.on = false
        local conns = DATA[cd.sig].conns
        for i = 1, #conns do
            if conns[i] == conn then table.remove(conns, i); break end
        end
    end

    local handlers = {}                 -- every function connected to a signal (the exploring part runs them)
    rt.handlers = handlers
    function rt.note_handler(sig, fn)
        if #handlers < 4000 then handlers[#handlers + 1] = {sig = sig, fn = fn} end
    end

    local function connect(sig, fn, once)
        local sd = DATA[sig]
        if type(fn) ~= 'function' then error('Attempt to connect failed: Passed value is not a function', 3) end
        local conn = newobj('RBXScriptConnection', {sig = sig, fn = fn, on = true, once = once})
        sd.conns[#sd.conns + 1] = conn
        emit('conn', sd.expr() .. (once and ':Once(' or ':Connect(') .. repr(fn) .. ')')
        rt.note_handler(sig, fn)
        if sd.frame then rt.ensure_ticker() end
        return conn
    end

    rt.connect = connect
    local SIGM = {
        Connect = function(sig, fn) return connect(sig, fn, false) end,
        ConnectParallel = function(sig, fn) return connect(sig, fn, false) end,
        Once = function(sig, fn) return connect(sig, fn, true) end,
        Wait = function(sig)
            local sd = DATA[sig]
            sd.waiters[#sd.waiters + 1] = coroutine.running()
            if sd.frame then rt.ensure_ticker() end
            return rt.park()
        end,
    }
    SIGM.connect = SIGM.Connect
    SIGM.wait = SIGM.Wait
    defkind('RBXScriptSignal', {
        __index = function(self, k)
            local f = SIGM[k]
            if f then return f end
            error(tostring(k) .. ' is not a valid member of RBXScriptSignal', 2)
        end,
        __tostring = function(self) return 'Signal ' .. tostring(DATA[self].name) end,
    })
    rt.REPR.RBXScriptSignal = function(self) return DATA[self].expr() end
    defkind('RBXScriptConnection', {
        __index = function(self, k)
            if k == 'Connected' then return DATA[self].on end
            if k == 'Disconnect' or k == 'disconnect' then
                return function(c) if KIND[c] == 'RBXScriptConnection' then rt.disconnect(c) end end
            end
            error(tostring(k) .. ' is not a valid member of RBXScriptConnection', 2)
        end,
        __tostring = function() return 'RBXScriptConnection' end,
    })
    rt.REPR.RBXScriptConnection = function() return 'connection' end

    -- ------------------------------------------------------------------------------------------ the tree
    local function attach(parent, child)
        local pd, cd = DATA[parent], DATA[child]
        cd.parent = parent
        pd.kids[#pd.kids + 1] = child
    end
    rt.attach = attach

    local function copy(t)
        local o = {}
        for i = 1, #t do o[i] = t[i] end
        return o
    end

    local function find_child(d, name, recursive)
        local kids = d.kids
        for i = 1, #kids do
            if DATA[kids[i]].name == name then return kids[i] end
        end
        if recursive then
            for i = 1, #kids do
                local f = find_child(DATA[kids[i]], name, true)
                if f then return f end
            end
        end
        return nil
    end
    rt.find_child = find_child

    local function reparent(inst, np)
        local d = DATA[inst]
        local old = d.parent
        if old == np then return end
        if np ~= nil then
            local c, guard = np, 0
            while c ~= nil and guard < 200 do
                if c == inst then error('Attempt to set ' .. fullname(inst) .. ' as its own parent', 3) end
                c = DATA[c].parent
                guard = guard + 1
            end
        end
        if old ~= nil then
            local ok = DATA[old].kids
            for i = 1, #ok do
                if ok[i] == inst then table.remove(ok, i); break end
            end
        end
        d.parent = np
        if np ~= nil then
            local pd = DATA[np]
            pd.kids[#pd.kids + 1] = inst
            local w = pd.waiters
            if w then
                for i = #w, 1, -1 do
                    local x = w[i]
                    if x.name == d.name and not x.done then
                        x.done = true
                        table.remove(w, i)
                        rt.wake(x.co, inst)
                    end
                end
            end
        end
        if old ~= nil then
            fire_if(old, 'ChildRemoved', inst)
            local a, guard = old, 0
            while a ~= nil and guard < 200 do
                fire_if(a, 'DescendantRemoving', inst)
                a = DATA[a].parent
                guard = guard + 1
            end
        end
        if np ~= nil then
            fire_if(np, 'ChildAdded', inst)
            local a, guard = np, 0
            while a ~= nil and guard < 200 do
                fire_if(a, 'DescendantAdded', inst)
                a = DATA[a].parent
                guard = guard + 1
            end
        end
        fire_if(inst, 'AncestryChanged', inst, np)
        local ps = d.psigs and d.psigs.Parent
        if ps then fire(ps) end
        fire_if(inst, 'Changed', 'Parent')
    end
    rt.reparent = reparent

    -- a guess at the class of a child the script asks for by name
    local NAME_CLASS = {
        Humanoid = 'Humanoid', HumanoidRootPart = 'Part', Head = 'Part', Torso = 'Part', UpperTorso = 'Part', LowerTorso = 'Part',
        ['Left Arm'] = 'Part', ['Right Arm'] = 'Part', ['Left Leg'] = 'Part', ['Right Leg'] = 'Part', LeftHand = 'Part', RightHand = 'Part',
        LeftFoot = 'Part', RightFoot = 'Part', LeftUpperArm = 'Part', RightUpperArm = 'Part', LeftLowerArm = 'Part', RightLowerArm = 'Part',
        LeftUpperLeg = 'Part', RightUpperLeg = 'Part', LeftLowerLeg = 'Part', RightLowerLeg = 'Part', Handle = 'Part',
        PlayerGui = 'PlayerGui', Backpack = 'Folder', PlayerScripts = 'Folder', Camera = 'Camera', Terrain = 'Terrain',
        Animator = 'Animator', Animate = 'LocalScript', Face = 'Decal', Neck = 'Motor6D', RootJoint = 'Motor6D',
        Chat = 'Folder', ChatMain = 'ModuleScript',
    }
    local function guess_class(name, parent_cls)
        return NAME_CLASS[name] or 'Folder'
    end
    rt.guess_class = guess_class

    -- a child that the script asks for and the pretend world has to make up
    local function auto_child(parent, name, style, cls)
        local pd = DATA[parent]
        local c = cls
        if c == nil then
            if pd.cls == 'Stats' or pd.cls == 'StatsItem' then c = 'StatsItem' else c = guess_class(name, pd.cls) end
        end
        local u, d = newinst(c, {name = name, world = true, auto = true})
        mark_child(u, parent, style, name)
        attach(parent, u)
        return u
    end
    rt.auto_child = auto_child

    -- ------------------------------------------------------------------------------------------ reading and writing
    local function absolute_size(inst, d, depth)
        local sz = d.props.Size
        local pw, ph = 1920, 1080
        local p = d.parent
        if p and depth < 20 and isa(DATA[p].cls, 'GuiObject') then
            local a = absolute_size(p, DATA[p], depth + 1)
            pw, ph = a[1], a[2]
        end
        if sz == nil then return {100, 100} end
        local s = DATA[sz]
        return {s.xs * pw + s.xo, s.ys * ph + s.yo}
    end
    local SPECIAL = {
        Position = function(self, d)
            if isa(d.cls, 'BasePart') then
                local cf = DATA[d.props.CFrame or DEF.BasePart.CFrame]
                return V3(cf.x, cf.y, cf.z)
            end
        end,
        AbsoluteSize = function(self, d)
            if isa(d.cls, 'GuiBase2d') then
                if isa(d.cls, 'GuiObject') then local a = absolute_size(self, d, 0); return V2(a[1], a[2]) end
                return V2(1920, 1080)
            end
        end,
        AbsolutePosition = function(self, d)
            if isa(d.cls, 'GuiBase2d') then return V2(0, 0) end
        end,
    }

    local function member_missing(self, d, key)
        if key == 'Value' or key == 'Count' or key == 'Amount' then return 0 end
        if d.world and not d.strict then
            if PROPLIKE[key] then return rt.ghost(self, dot(key), key) end
            return auto_child(self, key, 'index')
        end
        return rt.ghost(self, dot(key), key)
    end

    local mcache = {}
    local function methods_for(cls)
        local m = mcache[cls]
        if m then return m end
        local chain, c = {}, cls
        while c do chain[#chain + 1] = c; c = SUPER[c] end
        chain[#chain + 1] = 'Instance'
        m = {}
        for i = #chain, 1, -1 do
            local t = rt.CM[chain[i]]
            if t then for k, f in next, t do m[k] = f end end
        end
        mcache[cls] = m
        return m
    end
    rt.CM = {}

    -- which signals a class has (an unknown class has none: the name is then a child or a Ghost)
    local CLASS_SIGNALS = {}
    for cls, list in next, {
        Instance = 'Changed ChildAdded ChildRemoved DescendantAdded DescendantRemoving AncestryChanged Destroying AttributeChanged',
        BasePart = 'Touched TouchEnded',
        GuiObject = 'InputBegan InputChanged InputEnded MouseEnter MouseLeave MouseMoved MouseWheelForward MouseWheelBackward '
            .. 'SelectionGained SelectionLost TouchTap TouchLongPress TouchPan TouchPinch TouchRotate TouchSwipe',
        GuiButton = 'Activated MouseButton1Click MouseButton1Down MouseButton1Up MouseButton2Click MouseButton2Down MouseButton2Up',
        TextBox = 'FocusLost Focused ReturnPressedFromOnScreenKeyboard',
        Humanoid = 'Died HealthChanged Running Jumping Climbing Swimming Seated FreeFalling FallingDown GettingUp PlatformStanding '
            .. 'StateChanged MoveToFinished StateEnabledChanged Strafing Touched',
        Player = 'CharacterAdded CharacterRemoving CharacterAppearanceLoaded Chatted Idled OnTeleport',
        Players = 'PlayerAdded PlayerRemoving PlayerMembershipChanged',
        RunService = 'Heartbeat RenderStepped Stepped PreRender PreAnimation PreSimulation PostSimulation',
        UserInputService = 'InputBegan InputChanged InputEnded TextBoxFocused TextBoxFocusReleased WindowFocused WindowFocusReleased '
            .. 'JumpRequest LastInputTypeChanged TouchTap TouchPan TouchPinch TouchLongPress TouchSwipe TouchStarted TouchMoved TouchEnded '
            .. 'PointerAction UserInputTypeChanged',
        BaseRemoteEvent = 'OnClientEvent OnServerEvent',
        BindableEvent = 'Event',
        ClickDetector = 'MouseClick MouseHoverEnter MouseHoverLeave RightMouseClick',
        ProximityPrompt = 'Triggered TriggerEnded PromptShown PromptHidden PromptButtonHoldBegan PromptButtonHoldEnded',
        Tool = 'Equipped Unequipped Activated Deactivated',
        Tween = 'Completed',
        AnimationTrack = 'Stopped Ended DidLoop KeyframeReached',
        Animator = 'AnimationPlayed',
        Sound = 'Played Paused Resumed Stopped Ended Loaded DidLoop',
        Mouse = 'Button1Down Button1Up Button2Down Button2Up Idle Move WheelForward WheelBackward KeyDown KeyUp',
        DataModel = 'Loaded',
        GuiService = 'MenuOpened MenuClosed ErrorMessageChanged',
        ScriptContext = 'Error',
        LogService = 'MessageOut',
        TeleportService = 'LocalPlayerArrivedFromTeleport TeleportInitFailed',
        MarketplaceService = 'PromptPurchaseFinished PromptGamePassPurchaseFinished PromptProductPurchaseFinished '
            .. 'PromptBundlePurchaseFinished ThirdPartyPurchaseFinished PromptPremiumPurchaseFinished',
        ProximityPromptService = 'PromptTriggered PromptTriggerEnded PromptShown PromptHidden PromptButtonHoldBegan PromptButtonHoldEnded',
        ContextActionService = 'LocalToolEquipped LocalToolUnequipped',
        Chat = 'Chatted',
        TextChatService = 'MessageReceived SendingMessage',
        NetworkClient = 'ConnectionAccepted ConnectionRejected ConnectionFailed',
        Camera = 'InterpolationFinished',
        Seat = 'ChildAdded',
    } do
        local set = {}
        for w in string.gmatch(list, '%a+') do set[w] = true end
        CLASS_SIGNALS[cls] = set
    end
    local sigmemo = {}
    local function has_signal(cls, key)
        local m = sigmemo[cls]
        if m == nil then m = {}; sigmemo[cls] = m end
        local r = m[key]
        if r ~= nil then return r end
        r = false
        local c = cls
        while c do
            local set = CLASS_SIGNALS[c]
            if set and set[key] then r = true; break end
            c = SUPER[c]
        end
        if not r and CLASS_SIGNALS.Instance[key] then r = true end
        m[key] = r
        return r
    end
    rt.has_signal = has_signal

    -- a child that the sandbox made up as a Folder turns into what the script treats it as the first time it is used like it
    local DUCK = {}
    for cls, list in next, {
        Part = 'Position CFrame Size Touched TouchEnded Anchored CanCollide CanTouch CanQuery Transparency Orientation Velocity '
            .. 'AssemblyLinearVelocity AssemblyAngularVelocity Material BrickColor Mass Reflectance CastShadow Massless',
        RemoteEvent = 'OnClientEvent OnServerEvent',
        ClickDetector = 'MouseClick MouseHoverEnter MouseHoverLeave RightMouseClick MaxActivationDistance',
        ProximityPrompt = 'Triggered TriggerEnded HoldDuration ActionText ObjectText RequiresLineOfSight',
        Humanoid = 'Health MaxHealth WalkSpeed JumpPower JumpHeight HipHeight Died',
        TextButton = 'MouseButton1Click Activated MouseButton1Down MouseButton1Up',
    } do
        for w in string.gmatch(list, '%a+') do DUCK[w] = cls end
    end

    local function index(self, key)
        local d = DATA[self]
        local v = d.props[key]
        if v ~= nil then return v end
        if type(key) ~= 'string' then return nil end
        if key == 'Name' then return d.name end
        if key == 'ClassName' then return d.cls end
        if key == 'Parent' then return d.parent end
        if d.auto and d.cls == 'Folder' then
            local up = DUCK[key]
            if up then d.cls = up end
        end
        local sp = SPECIAL[key]
        if sp then
            v = sp(self, d)
            if v ~= nil then return v end
        end
        local m = methods_for(d.cls)[key]
        if m then return m end
        if SIGNALS[key] and has_signal(d.cls, key) then return getsig(self, d, key) end
        v = default(d.cls, key)
        if v ~= nil then return v end
        local ch = find_child(d, key, false)
        if ch then
            if DATA[ch].how == nil then mark_child(ch, self, 'index', key) end
            return ch
        end
        if d.hook_index then
            v = d.hook_index(self, d, key)
            if v == rt.NIL then return nil end
            if v ~= nil then return v end
        end
        if SIGNALS[key] and not d.world then return getsig(self, d, key) end
        return member_missing(self, d, key)
    end

    -- a property changed: the signals say so (the Changed event of a Value object passes the new value, others the name)
    local function changed(self, d, key, val)
        local ps = d.psigs and d.psigs[key]
        if ps then fire(ps) end
        if isa(d.cls, 'ValueBase') then
            if key == 'Value' then fire_if(self, 'Changed', val) end
        else
            fire_if(self, 'Changed', key)
        end
    end
    rt.changed = changed

    -- change a property without writing it to the log (what the sandbox itself does: tweens, damage, ...)
    function rt.setprop(inst, key, val)
        local d = DATA[inst]
        if key == 'Name' then d.name = tostring(val); return end
        if key == 'Parent' then rt.reparent(inst, val); return end
        local old = d.props[key]
        d.props[key] = val
        if old ~= val then changed(inst, d, key, val) end
    end

    local function newindex(self, key, val)
        local d = DATA[self]
        if type(key) ~= 'string' then error('Invalid property name: ' .. tostring(key), 2) end
        if key == 'Parent' then
            if val ~= nil and KIND[val] ~= 'Instance' then
                error('Unable to assign property Parent. Instance expected, got ' .. rt.typeof(val), 2)
            end
            emit('set', ref(self) .. '.Parent = ' .. repr(val, 1))
            reparent(self, val)
            return
        end
        emit('set', ref(self) .. dot(key) .. ' = ' .. repr(val, 1))
        local old
        if key == 'Name' then
            old = d.name
            d.name = tostring(val)
        elseif key == 'Position' and isa(d.cls, 'BasePart') and KIND[val] == 'Vector3' then
            old = d.props.CFrame
            local cf = DATA[old or DEF.BasePart.CFrame]
            local p = DATA[val]
            d.props.CFrame = CF(p.X, p.Y, p.Z, cf.r)
            key = 'CFrame'
        else
            old = d.props[key]
            d.props[key] = val
        end
        if old ~= val or d.always then changed(self, d, key, val) end
        if d.hook_set then d.hook_set(self, d, key, val) end
    end

    -- a call of an Instance (`obj.Method(obj, ...)` where Method is a name the sandbox does not know): written as a
    -- method call
    local function icall(self, ...)
        local d = DATA[self]
        local n = select('#', ...)
        local args = pack(...)
        local pr = d.parent
        if d.auto and pr ~= nil then
            -- `part:SomeMethod()` of a method the sandbox does not know: it was made up as a child, take it back
            local ok = DATA[pr].kids
            for i = #ok, 1, -1 do
                if ok[i] == self then table.remove(ok, i) end
            end
            d.parent = nil
            d.auto = nil
        end
        local text
        if pr ~= nil and n >= 1 and rawequal(args[1], pr) then
            local rest = {n = n - 1, unpack(args, 2, n)}
            text = ref(pr) .. ':' .. d.name .. '(' .. rt.args(rest, n - 1) .. ')'
        else
            text = ref(self) .. '(' .. rt.args(args, n) .. ')'
        end
        emit('call', text)
        return rt.ghost(self, '(' .. rt.args(args, n) .. ')')
    end

    defkind('Instance', {
        __index = index,
        __newindex = newindex,
        __call = icall,
        __tostring = function(self) return DATA[self].name end,
    })
    rt.index_instance = index

    -- ------------------------------------------------------------------------------------------ methods
    local KINDOF = {FireServer = 'remote', InvokeServer = 'remote', FireClient = 'remote', FireAllClients = 'remote', InvokeClient = 'remote',
        Kick = 'kick', Teleport = 'tp', TeleportToPlaceInstance = 'tp', TeleportAsync = 'tp', TeleportPartyAsync = 'tp',
        TeleportToSpawnByName = 'tp', Connect = 'conn', Once = 'conn', ConnectParallel = 'conn'}
    local function is_quiet(name)
        return string.find(name, '^Find') or string.find(name, '^Get') or string.find(name, '^Is') or string.find(name, '^Has')
            or string.find(name, '^Wait')
    end
    -- methods whose result is a new object: the call is written as `local Name = obj:Method(...)`
    local CREATORS = {Create = true, Clone = true, LoadAnimation = true, GetMouse = true, CreatePath = true, LoadAsset = true,
        GetHumanoidDescriptionFromUserId = true, GetHumanoidDescriptionFromOutfitId = true}
    rt.CREATORS = CREATORS

    local function made(text, kind, r1, ...)
        if KIND[r1] == 'Instance' and DATA[r1].how == nil then
            local rd = DATA[r1]
            local var = newvar(rd.name)
            rd.var = var
            rd.how = function() return text end
            emit('def', 'local ' .. var .. ' = ' .. text, rd.cls)
        else
            emit(kind, text)
        end
        return r1, ...
    end
    local function post(text, r1, ...)
        if KIND[r1] == 'Instance' and DATA[r1].how == nil then
            local rd = DATA[r1]
            rd.how = function() return text end
        end
        return r1, ...
    end

    -- define a method of a class.  fn(self, data, ...).  The call is written to the log unless the name says it only asks
    local function defm(cls, name, fn, quiet)
        local q = quiet or is_quiet(name)
        local kind = KINDOF[name] or 'call'
        local creator = CREATORS[name]
        local t = rt.CM[cls]
        if not t then t = {}; rt.CM[cls] = t end
        t[name] = function(self, ...)
            if KIND[self] ~= 'Instance' then error("Expected ':' not '.' calling member function " .. name, 2) end
            if q then return fn(self, DATA[self], ...) end
            local text = ref(self) .. ':' .. name .. '(' .. rt.args(pack(...)) .. ')'
            if creator then return made(text, kind, fn(self, DATA[self], ...)) end
            if kind == 'remote' then emit(kind, text, fullname(self), name) else emit(kind, text) end
            return post(text, fn(self, DATA[self], ...))
        end
    end
    rt.defm = defm

    -- remote calls work on any instance, whatever its class; the answer of the other side is not known
    for name in pairs(KINDOF) do
        if KINDOF[name] == 'remote' then defm('Instance', name, function(self, d, ...)
            return rt.ghost(self, ':' .. name .. '(' .. rt.args(pack(...)) .. ')')
        end) end
    end
    -- a child that was made up and that the script uses like a signal (`Remotes.Event:Connect(f)`, `Door.Touched:Connect(f)`)
    local function dual(self, d)
        local s = d.dualsig
        if not s then
            s = newsig(nil, d.name, function() return ref(self) end)
            d.dualsig = s
        end
        return s
    end
    defm('Instance', 'Connect', function(self, d, fn) return connect(dual(self, d), fn, false) end, true)
    defm('Instance', 'ConnectParallel', function(self, d, fn) return connect(dual(self, d), fn, false) end, true)
    defm('Instance', 'Once', function(self, d, fn) return connect(dual(self, d), fn, true) end, true)
    defm('Instance', 'Wait', function(self, d)
        local sd = DATA[dual(self, d)]
        sd.waiters[#sd.waiters + 1] = coroutine.running()
        return rt.park()
    end, true)

    DEF.Instance.RobloxLocked = false

    local function kill(inst)
        local d = DATA[inst]
        if d.destroyed then return end
        d.destroyed = true
        fire_if(inst, 'Destroying')
        local kids = copy(d.kids)
        for i = 1, #kids do kill(kids[i]) end
    end
    -- what Destroy does to the tree: everything below ends up without a parent and the signals of the instance go quiet
    local function unhook(inst)
        local d = DATA[inst]
        local kids = copy(d.kids)
        for i = 1, #kids do unhook(kids[i]) end
        if d.parent ~= nil then reparent(inst, nil) end
        if d.sigs then
            for _, s in next, d.sigs do
                local conns = DATA[s].conns
                for j = #conns, 1, -1 do DATA[conns[j]].on = false; conns[j] = nil end
            end
        end
    end
    local function destroy(self, d)
        kill(self)
        reparent(self, nil)
        unhook(self)
    end
    rt.destroy = destroy
    rt.NIL = {}                 -- a hook_index function returns this when the answer is nil
    defm('Instance', 'Destroy', destroy)
    defm('Instance', 'Remove', destroy)
    defm('Instance', 'destroy', destroy)
    defm('Instance', 'remove', destroy)
    defm('Instance', 'ClearAllChildren', function(self, d)
        local kids = copy(d.kids)
        for i = 1, #kids do destroy(kids[i], DATA[kids[i]]) end
    end)

    local function clone(self, d)
        local u, nd = newinst(d.cls, {name = d.name, world = d.world})
        for k, v in next, d.props do nd.props[k] = v end
        if d.attrs then nd.attrs = {}; for k, v in next, d.attrs do nd.attrs[k] = v end end
        for i = 1, #d.kids do
            local kc = clone(d.kids[i], DATA[d.kids[i]])
            attach(u, kc)
        end
        return u
    end
    rt.clone = clone
    defm('Instance', 'Clone', function(self, d) return clone(self, d) end)
    rt.CM.Instance.clone = rt.CM.Instance.Clone

    local function descend(d, out)
        for i = 1, #d.kids do
            out[#out + 1] = d.kids[i]
            descend(DATA[d.kids[i]], out)
        end
        return out
    end
    defm('Instance', 'GetChildren', function(self, d)
        local out = copy(d.kids)
        for i = 1, #out do
            local kd = DATA[out[i]]
            if kd.how == nil then
                local idx = i
                kd.how = function() return ref(self) .. ':GetChildren()[' .. idx .. ']' end
            end
        end
        return out
    end)
    rt.CM.Instance.getChildren = rt.CM.Instance.GetChildren
    rt.CM.Instance.children = rt.CM.Instance.GetChildren
    defm('Instance', 'GetDescendants', function(self, d)
        local out = descend(d, {})
        for i = 1, #out do
            local kd = DATA[out[i]]
            if kd.how == nil then
                local idx = i
                kd.how = function() return ref(self) .. ':GetDescendants()[' .. idx .. ']' end
            end
        end
        return out
    end)
    defm('Instance', 'GetFullName', function(self, d) return fullname(self) end)
    defm('Instance', 'GetDebugId', function(self, d) return sfmt('%08x-0000-0000-0000-%012x', d.id, d.id) end)
    local function isa_method(self, d, c) return type(c) == 'string' and isa(d.cls, c) end
    defm('Instance', 'IsA', isa_method)
    defm('Instance', 'isA', isa_method)
    defm('Instance', 'IsDescendantOf', function(self, d, a)
        local c, guard = d.parent, 0
        while c ~= nil and guard < 200 do
            if c == a then return true end
            c = DATA[c].parent
            guard = guard + 1
        end
        return false
    end)
    defm('Instance', 'IsAncestorOf', function(self, d, x)
        if KIND[x] ~= 'Instance' then return false end
        local c, guard = DATA[x].parent, 0
        while c ~= nil and guard < 200 do
            if c == self then return true end
            c = DATA[c].parent
            guard = guard + 1
        end
        return false
    end)

    -- looking for children.  In the pretend world a name that is not there is made up.
    local function found(self, d, kid, style, name)
        local kd = DATA[kid]
        if kd.how == nil then mark_child(kid, self, style, name) end
        return kid
    end
    -- GUI containers (PlayerGui, CoreGui, StarterGui): scripts put their own windows here and often look for an old copy
    -- first ("already loaded, stop").  So a search that finds nothing says so; WaitForChild and plain indexing still
    -- make the game's own windows up, because a script that does that expects them to exist.
    local CORE_GUIS = {RobloxGui = true, RobloxPromptGui = true, TopBarApp = true, PlayerList = true, Chat = true, ExperienceChat = true,
        PurchasePromptApp = true, ThemeProvider = true, RobloxNetworkPauseNotification = true, DevConsoleMaster = true,
        Backpack = true, TeleportEffect = true, TeleportGui = true, FoundationOverlay = true, ControlGui = true, PlayerListMaster = true}
    local function makes_up(d, name)
        if not d.world or d.strict then return false end
        if isa(d.cls, 'BasePlayerGui') then return type(name) == 'string' and d.cls == 'CoreGui' and CORE_GUIS[name] == true end
        return true
    end
    defm('Instance', 'FindFirstChild', function(self, d, name, recursive)
        local k = find_child(d, name, recursive)
        if k then return found(self, d, k, 'FindFirstChild', name) end
        if type(name) == 'string' and makes_up(d, name) then return auto_child(self, name, 'FindFirstChild') end
        return nil
    end)
    rt.CM.Instance.findFirstChild = rt.CM.Instance.FindFirstChild
    defm('Instance', 'FindFirstChildOfClass', function(self, d, cls)
        for i = 1, #d.kids do
            if DATA[d.kids[i]].cls == cls then return found(self, d, d.kids[i], 'FindFirstChildOfClass', cls) end
        end
        if type(cls) == 'string' and makes_up(d, nil) and cls ~= 'Humanoid' then
            return auto_child(self, cls, 'FindFirstChildOfClass', cls)
        end
        if d.world and cls == 'Humanoid' then return auto_child(self, 'Humanoid', 'FindFirstChildOfClass', 'Humanoid') end
        return nil
    end)
    defm('Instance', 'FindFirstChildWhichIsA', function(self, d, cls, recursive)
        local list = recursive and descend(d, {}) or d.kids
        for i = 1, #list do
            if isa(DATA[list[i]].cls, cls) then return found(self, d, list[i], 'FindFirstChildWhichIsA', cls) end
        end
        if type(cls) == 'string' and makes_up(d, nil) then return auto_child(self, cls, 'FindFirstChildWhichIsA', cls) end
        return nil
    end)
    defm('Instance', 'FindFirstDescendant', function(self, d, name)
        local k = find_child(d, name, true)
        if k then return found(self, d, k, 'FindFirstDescendant', name) end
        return nil
    end)
    local function ancestor(self, d, test)
        local c, guard = d.parent, 0
        while c ~= nil and guard < 200 do
            if test(DATA[c]) then return c end
            c = DATA[c].parent
            guard = guard + 1
        end
        return nil
    end
    defm('Instance', 'FindFirstAncestor', function(self, d, name) return ancestor(self, d, function(x) return x.name == name end) end)
    defm('Instance', 'FindFirstAncestorOfClass', function(self, d, cls) return ancestor(self, d, function(x) return x.cls == cls end) end)
    defm('Instance', 'FindFirstAncestorWhichIsA', function(self, d, cls) return ancestor(self, d, function(x) return isa(x.cls, cls) end) end)
    defm('Instance', 'WaitForChild', function(self, d, name, timeout)
        if type(name) ~= 'string' then error('Argument 1 missing or nil', 3) end
        local k = find_child(d, name, false)
        if k then return found(self, d, k, 'WaitForChild', name) end
        if d.world then return auto_child(self, name, 'WaitForChild') end
        -- the script's own instance: wait until the child shows up (or the time is over)
        local co = coroutine.running()
        local w = {co = co, name = name}
        d.waiters = d.waiters or {}
        d.waiters[#d.waiters + 1] = w
        if timeout then
            rt.later(timeout, function()
                if not w.done then
                    w.done = true
                    rt.wake(co, nil)
                end
            end)
        else
            rt.later(5, function()
                if not w.done then
                    emit('note', 'infinite yield: ' .. fullname(self) .. ':WaitForChild(' .. quote(name) .. ') has been waiting for 5 s; the child does not exist in this sandbox')
                end
            end)
        end
        return rt.park()
    end)

    -- attributes, tags
    defm('Instance', 'SetAttribute', function(self, d, k, v)
        d.attrs = d.attrs or {}
        local old = d.attrs[k]
        d.attrs[k] = v
        if old ~= v then
            fire_if(self, 'AttributeChanged', k)
            local s = d.asigs and d.asigs[k]
            if s then fire(s) end
        end
    end)
    defm('Instance', 'GetAttribute', function(self, d, k) return d.attrs and d.attrs[k] or nil end)
    defm('Instance', 'GetAttributes', function(self, d)
        local o = {}
        if d.attrs then for k, v in next, d.attrs do o[k] = v end end
        return o
    end)
    defm('Instance', 'GetAttributeChangedSignal', function(self, d, k)
        d.asigs = d.asigs or {}
        local s = d.asigs[k]
        if not s then
            s = newsig(self, k, function() return ref(self) .. ':GetAttributeChangedSignal(' .. quote(tostring(k)) .. ')' end)
            d.asigs[k] = s
        end
        return s
    end)
    defm('Instance', 'GetPropertyChangedSignal', function(self, d, k)
        d.psigs = d.psigs or {}
        local s = d.psigs[k]
        if not s then
            s = newsig(self, k, function() return ref(self) .. ':GetPropertyChangedSignal(' .. quote(tostring(k)) .. ')' end)
            d.psigs[k] = s
        end
        return s
    end)
    local tags = {}
    rt.tags = tags
    defm('Instance', 'AddTag', function(self, d, tag)
        tags[tag] = tags[tag] or {}
        tags[tag][self] = true
    end)
    defm('Instance', 'RemoveTag', function(self, d, tag) if tags[tag] then tags[tag][self] = nil end end)
    defm('Instance', 'HasTag', function(self, d, tag) return tags[tag] ~= nil and tags[tag][self] == true end)
    defm('Instance', 'GetTags', function(self, d)
        local o = {}
        for t, set in next, tags do if set[self] then o[#o + 1] = t end end
        return o
    end)

    -- Bindable events and functions work for real
    defm('BindableEvent', 'Fire', function(self, d, ...) fire(getsig(self, d, 'Event'), ...) end)
    defm('BindableFunction', 'Invoke', function(self, d, ...)
        local cb = d.props.OnInvoke
        if type(cb) == 'function' then return cb(...) end
        return nil
    end)
    DEF.BindableEvent = {}

    -- ------------------------------------------------------------------------------------------ Instance.new
    rt.Instance = {
        new = function(cls, parent)
            if type(cls) ~= 'string' or not string.find(cls, '^%a[%w]*$') then
                error('Unable to create an Instance of type "' .. tostring(cls) .. '"', 2)
            end
            if parent ~= nil and KIND[parent] ~= 'Instance' then error('Argument 2 missing or nil', 2) end
            local u, d = newinst(cls, {name = cls, world = false})
            local var = newvar(cls)
            d.var = var
            local text = 'Instance.new(' .. quote(cls) .. (parent ~= nil and (', ' .. ref(parent)) or '') .. ')'
            emit('def', 'local ' .. var .. ' = ' .. text, cls)
            if parent ~= nil then reparent(u, parent) end
            return u
        end,
        fromExisting = function(inst) return clone(inst, DATA[inst]) end,
    }
end
