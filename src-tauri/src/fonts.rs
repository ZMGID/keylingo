//! 枚举系统已安装字体家族名，供设置里的「界面字体」搜索选择。
//! 前端无法枚举（WKWebView 不支持 queryLocalFonts），故走原生：
//! macOS = CoreText，Windows = GDI，其余平台返回空。复用现有 core-foundation / windows 依赖，不新增。

#[tauri::command]
pub fn list_system_fonts() -> Vec<String> {
    let mut names = enumerate_system_fonts();
    // 去掉系统隐藏字体（. 前缀，如 .AppleSystemUIFont）；去重 + 不区分大小写排序。
    names.retain(|n| !n.is_empty() && !n.starts_with('.'));
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

#[cfg(target_os = "macos")]
fn enumerate_system_fonts() -> Vec<String> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        fn CTFontManagerCopyAvailableFontFamilyNames() -> CFArrayRef;
    }

    let mut out = Vec::new();
    unsafe {
        let arr_ref = CTFontManagerCopyAvailableFontFamilyNames();
        if arr_ref.is_null() {
            return out;
        }
        let arr: CFArray<CFString> = CFArray::wrap_under_create_rule(arr_ref);
        for name in arr.iter() {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn enumerate_system_fonts() -> Vec<String> {
    use std::collections::BTreeSet;
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Graphics::Gdi::{
        EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
    };

    unsafe extern "system" fn cb(
        lf: *const LOGFONTW,
        _tm: *const TEXTMETRICW,
        _font_type: u32,
        lparam: LPARAM,
    ) -> i32 {
        let set = &mut *(lparam.0 as *mut BTreeSet<String>);
        let face = &(*lf).lfFaceName;
        let len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let name = String::from_utf16_lossy(&face[..len]);
        // @ 前缀是竖排 CJK 变体，跳过。
        if !name.is_empty() && !name.starts_with('@') {
            set.insert(name);
        }
        1
    }

    let mut set: BTreeSet<String> = BTreeSet::new();
    unsafe {
        let hdc = GetDC(None);
        let mut lf = LOGFONTW::default();
        lf.lfCharSet = DEFAULT_CHARSET;
        EnumFontFamiliesExW(hdc, &lf, Some(cb), LPARAM(&mut set as *mut _ as isize), 0);
        ReleaseDC(None, hdc);
    }
    set.into_iter().collect()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn enumerate_system_fonts() -> Vec<String> {
    Vec::new()
}
