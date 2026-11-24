// Property tables shared by the Lingo decompiler (number -> name) and the Lingo compiler (name -> number).
//
// The numbers are the operands of the Director 4 style `get`/`set` instructions. The names follow
// ProjectorRays' lingodec/names.cpp (MPL-2.0), which the decompiler in ../decompiler was ported from.

pub const MOVIE_PROPS: &[(u8, &str)] = &[
    (0x00, "floatPrecision"),
    (0x01, "mouseDownScript"),
    (0x02, "mouseUpScript"),
    (0x03, "keyDownScript"),
    (0x04, "keyUpScript"),
    (0x05, "timeoutScript"),
    (0x06, "short time"),
    (0x07, "abbr time"),
    (0x08, "long time"),
    (0x09, "short date"),
    (0x0a, "abbr date"),
    (0x0b, "long date"),
];

pub const MENU_PROPS: &[(u8, &str)] = &[(0x01, "name"), (0x02, "number of menuItems")];

pub const MENU_ITEM_PROPS: &[(u8, &str)] = &[(0x01, "name"), (0x02, "checkMark"), (0x03, "enabled"), (0x04, "script")];

pub const SOUND_PROPS: &[(u8, &str)] = &[(0x01, "volume")];

pub const SPRITE_PROPS: &[(u8, &str)] = &[
    (0x01, "type"),
    (0x02, "backColor"),
    (0x03, "bottom"),
    (0x04, "castNum"),
    (0x05, "constraint"),
    (0x06, "cursor"),
    (0x07, "foreColor"),
    (0x08, "height"),
    (0x09, "immediate"),
    (0x0a, "ink"),
    (0x0b, "left"),
    (0x0c, "lineSize"),
    (0x0d, "locH"),
    (0x0e, "locV"),
    (0x0f, "movieRate"),
    (0x10, "movieTime"),
    (0x11, "pattern"),
    (0x12, "puppet"),
    (0x13, "right"),
    (0x14, "startTime"),
    (0x15, "stopTime"),
    (0x16, "stretch"),
    (0x17, "top"),
    (0x18, "trails"),
    (0x19, "visible"),
    (0x1a, "volume"),
    (0x1b, "width"),
    (0x1c, "blend"),
    (0x1d, "scriptNum"),
    (0x1e, "moveableSprite"),
    (0x1f, "editableText"),
    (0x20, "scoreColor"),
    (0x21, "loc"),
    (0x22, "rect"),
    (0x23, "memberNum"),
    (0x24, "castLibNum"),
    (0x25, "member"),
    (0x26, "scriptInstanceList"),
    (0x27, "currentTime"),
    (0x28, "mostRecentCuePoint"),
    (0x29, "tweened"),
    (0x2a, "name"),
];

pub const ANIM_PROPS: &[(u8, &str)] = &[
    (0x01, "beepOn"),
    (0x02, "buttonStyle"),
    (0x03, "centerStage"),
    (0x04, "checkBoxAccess"),
    (0x05, "checkboxType"),
    (0x06, "colorDepth"),
    (0x07, "colorQD"),
    (0x08, "exitLock"),
    (0x09, "fixStageSize"),
    (0x0a, "fullColorPermit"),
    (0x0b, "imageDirect"),
    (0x0c, "doubleClick"),
    (0x0d, "key"),
    (0x0e, "lastClick"),
    (0x0f, "lastEvent"),
    (0x10, "keyCode"),
    (0x11, "lastKey"),
    (0x12, "lastRoll"),
    (0x13, "timeoutLapsed"),
    (0x14, "multiSound"),
    (0x15, "pauseState"),
    (0x16, "quickTimePresent"),
    (0x17, "selEnd"),
    (0x18, "selStart"),
    (0x19, "soundEnabled"),
    (0x1a, "soundLevel"),
    (0x1b, "stageColor"),
    (0x1d, "switchColorDepth"),
    (0x1e, "timeoutKeyDown"),
    (0x1f, "timeoutLength"),
    (0x20, "timeoutMouse"),
    (0x21, "timeoutPlay"),
    (0x22, "timer"),
    (0x23, "preLoadRAM"),
    (0x24, "videoForWindowsPresent"),
    (0x25, "netPresent"),
    (0x26, "safePlayer"),
    (0x27, "soundKeepDevice"),
    (0x28, "soundMixMedia"),
];

pub const ANIM2_PROPS: &[(u8, &str)] = &[
    (0x01, "perFrameHook"),
    (0x02, "number of castMembers"),
    (0x03, "number of menus"),
    (0x04, "number of castLibs"),
    (0x05, "number of xtras"),
];

pub const MEMBER_PROPS: &[(u8, &str)] = &[
    (0x01, "name"),
    (0x02, "text"),
    (0x03, "textStyle"),
    (0x04, "textFont"),
    (0x05, "textHeight"),
    (0x06, "textAlign"),
    (0x07, "textSize"),
    (0x08, "picture"),
    (0x09, "hilite"),
    (0x0a, "number"),
    (0x0b, "size"),
    (0x0c, "loop"),
    (0x0d, "duration"),
    (0x0e, "controller"),
    (0x0f, "directToStage"),
    (0x10, "sound"),
    (0x11, "foreColor"),
    (0x12, "backColor"),
    (0x13, "type"),
];

/// The name for a property number, or `None` when the table has no such number.
pub fn name_of(table: &[(u8, &'static str)], id: i64) -> Option<&'static str> {
    table.iter().find(|(n, _)| *n as i64 == id).map(|(_, name)| *name)
}

/// The property number for a name (case-insensitive), or `None`.
pub fn id_of(table: &[(u8, &'static str)], name: &str) -> Option<u8> {
    table.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(id, _)| *id)
}

/// The member-property `get`/`set` types that all print as `the <prop> of member <id>`. The decompiler
/// writes the others with a `#<type>` note so that the compiler can give the same type back.
pub const MEMBER_TYPE_GENERIC: u8 = 0x09;
pub const MEMBER_TYPE_CHUNK: u8 = 0x0a;
pub const MEMBER_TYPE_FIELD: u8 = 0x0b;
pub const MEMBER_TYPE_FIELD_CHUNK: u8 = 0x0c;
pub const MEMBER_TYPE_SCRIPT_TEXT: u8 = 0x14;
pub const MEMBER_TYPE_SCRIPT_TEXT_CHUNK: u8 = 0x15;
