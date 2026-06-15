// user_layout.rs — ユーザー定義初期配列（initial_layout.toml）の読み込みと変換

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use crate::chars::{build_char_to_id, CharId, MAX_CHARS, VOID_CHAR_FIRST};
use crate::layout::{KeyboardParams, KeyboardSize, Layout, SlotId, SHIFT_SLOT_SENTINEL, MAX_SLOTS};

pub const USER_LAYOUT_PATH: &str = "initial_layout.toml";

/// initial_layout.toml のトップレベル構造
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserLayoutFile {
    pub layout_3x10: Option<UserLayoutDef>,
    pub layout_3x10_single_shift: Option<UserLayoutDef>,
    pub layout_3x11: Option<UserLayoutDef>,
}

/// 1レイアウト定義（Layer 1 / Layer 2 それぞれ3行の文字列）
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserLayoutDef {
    /// Layer 1: 3要素、各要素は num_cols 文字の文字列
    /// 3x11 では row1 のシフトキー位置（col2=☆, col7=★）に任意のプレースホルダーを置く
    pub layer1: Vec<String>,
    /// Layer 2: 3要素、各要素は num_cols 文字の文字列
    pub layer2: Vec<String>,
}

impl UserLayoutFile {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("initial_layout.toml 読み込みエラー: {}", e))?;
        toml::from_str(&text).map_err(|e| format!("initial_layout.toml パースエラー: {}", e))
    }

    /// キーボードサイズに対応するレイアウト定義を取得する
    pub fn get_def(&self, kp: KeyboardParams) -> Option<&UserLayoutDef> {
        match kp.size {
            KeyboardSize::K3x10 => self.layout_3x10.as_ref(),
            KeyboardSize::K3x10SingleShift => self.layout_3x10_single_shift.as_ref(),
            KeyboardSize::K3x11 => self.layout_3x11.as_ref(),
        }
    }
}

/// UserLayoutDef から Layout を構築する。
///
/// 文字の重複・欠落・行数・列数の不一致はすべて Err(message) として返す。
pub fn parse_user_layout(kp: KeyboardParams, def: &UserLayoutDef) -> Result<Layout, String> {
    let nc = kp.num_cols as usize;
    let npl = kp.num_slots_per_layer as usize;
    let char_map = build_char_to_id();

    if def.layer1.len() != 3 {
        return Err(format!(
            "layer1 は3行必要です（{}行）",
            def.layer1.len()
        ));
    }
    if def.layer2.len() != 3 {
        return Err(format!(
            "layer2 は3行必要です（{}行）",
            def.layer2.len()
        ));
    }

    // u8::MAX (= SHIFT_SLOT_SENTINEL) を「未割当」センチネルとして使用
    // 有効スロット ID は 0..65 なので衝突しない
    let mut cts = [u8::MAX; MAX_CHARS];
    let mut stc = [SHIFT_SLOT_SENTINEL; MAX_SLOTS];

    // 3x11 のシフトキースロットを明示的に SHIFT_SLOT_SENTINEL で初期化
    if kp.size == KeyboardSize::K3x11 {
        stc[kp.shift_left as usize] = SHIFT_SLOT_SENTINEL;
        stc[kp.shift_right as usize] = SHIFT_SLOT_SENTINEL;
    }

    // void 文字（□）への CharId 割り当てカウンタ
    let mut void_next: CharId = VOID_CHAR_FIRST;

    // Layer1 のパース
    for (row, line) in def.layer1.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() != nc {
            return Err(format!(
                "layer1 row{} は{}文字必要です（{}文字: {:?}）",
                row, nc, chars.len(), line
            ));
        }
        for (col, &c) in chars.iter().enumerate() {
            let slot = (row * nc + col) as SlotId;

            // 3x11: ☆★ 位置はシフトキー → 書かれた文字にかかわらずスキップ
            if kp.size == KeyboardSize::K3x11
                && (slot == kp.shift_left || slot == kp.shift_right)
            {
                stc[slot as usize] = SHIFT_SLOT_SENTINEL;
                continue;
            }

            assign_char(c, slot, &char_map, &mut cts, &mut stc, &mut void_next, row, col, "layer1")?;
        }
    }

    // Layer2 のパース
    for (row, line) in def.layer2.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() != nc {
            return Err(format!(
                "layer2 row{} は{}文字必要です（{}文字: {:?}）",
                row, nc, chars.len(), line
            ));
        }
        for (col, &c) in chars.iter().enumerate() {
            let slot = (npl + row * nc + col) as SlotId;
            assign_char(c, slot, &char_map, &mut cts, &mut stc, &mut void_next, row, col, "layer2")?;
        }
    }

    // バリデーション: 0..num_chars の全文字が有効スロットに1度ずつ配置されているか
    let num_chars = kp.num_chars;
    let mut slot_used = [false; MAX_SLOTS];
    for (c, &s) in cts[..num_chars].iter().enumerate() {
        if s == u8::MAX {
            let ch = if c < VOID_CHAR_FIRST as usize {
                crate::chars::CHAR_LIST[c].to_string()
            } else {
                format!("□(void {})", c)
            };
            return Err(format!(
                "文字 '{}' (CharId {}) が配置されていません",
                ch, c
            ));
        }
        if (s as usize) >= kp.num_slots {
            return Err(format!("CharId {} のスロット {} が範囲外です", c, s));
        }
        if slot_used[s as usize] {
            return Err(format!("スロット {} に複数の文字が割り当てられています", s));
        }
        slot_used[s as usize] = true;
    }

    if kp.size == crate::layout::KeyboardSize::K3x10SingleShift
        && cts[crate::chars::TOUTEN_ID as usize] != crate::layout::E_SHIFT_SLOT
    {
        return Err(format!(
            "3x10_single_shift では '、' は row0 col2 (slot {}) に配置してください",
            crate::layout::E_SHIFT_SLOT
        ));
    }

    Ok(Layout {
        kp,
        char_to_slot: cts,
        slot_to_char: stc,
    })
}

/// 1文字をスロットへ割り当てる（cts / stc を更新）
#[allow(clippy::too_many_arguments)]
fn assign_char(
    c: char,
    slot: SlotId,
    char_map: &HashMap<char, CharId>,
    cts: &mut [u8; MAX_CHARS],
    stc: &mut [CharId; MAX_SLOTS],
    void_next: &mut CharId,
    row: usize,
    col: usize,
    layer: &str,
) -> Result<(), String> {
    if c == '□' {
        // void 文字: CharId を動的に割り当て
        if *void_next as usize >= MAX_CHARS {
            return Err(format!(
                "void 文字（□）の数が上限を超えました（{} row{}, col{}）",
                layer, row, col
            ));
        }
        cts[*void_next as usize] = slot;
        stc[slot as usize] = *void_next;
        *void_next += 1;
        return Ok(());
    }

    let char_id = char_map.get(&c).copied().ok_or_else(|| {
        format!(
            "未知の文字 '{}' ({} row{}, col{})",
            c, layer, row, col
        )
    })?;

    // 重複チェック
    if cts[char_id as usize] != u8::MAX {
        return Err(format!(
            "文字 '{}' が重複しています（{} row{}, col{}）",
            c, layer, row, col
        ));
    }

    cts[char_id as usize] = slot;
    stc[slot as usize] = char_id;
    Ok(())
}
