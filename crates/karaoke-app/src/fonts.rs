use std::sync::Arc;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

/// `(path, face index within a collection)`, tried in order.
const CANDIDATES: [(&str, u32); 5] = [
    (r"C:\Windows\Fonts\msyh.ttc", 0),
    (r"C:\Windows\Fonts\simhei.ttf", 0),
    (r"C:\Windows\Fonts\NotoSansSC-VF.ttf", 0),
    ("/mnt/c/Windows/Fonts/msyh.ttc", 0),
    // Face 2 of the Noto CJK collection is the Simplified Chinese one.
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
];

/// Installs the first readable CJK font as a fallback for both families.
pub fn install(ctx: &Context) -> Result<(), String> {
    let Some((bytes, index)) = CANDIDATES
        .iter()
        .find_map(|&(path, index)| std::fs::read(path).ok().map(|b| (b, index)))
    else {
        return Err("No Chinese font found; install Microsoft YaHei or Noto Sans SC".into());
    };
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    let mut defs = FontDefinitions::default();
    defs.font_data.insert("cjk".into(), Arc::new(data));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        defs.families.entry(family).or_default().push("cjk".into());
    }
    ctx.set_fonts(defs);
    Ok(())
}
