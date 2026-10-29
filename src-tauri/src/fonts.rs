//! Enumerate font families visible to this Windows user, without reading font files.
use std::collections::BTreeMap;
use windows::Win32::{Foundation::LPARAM,Graphics::Gdi::{
    GetDC, ReleaseDC, EnumFontFamiliesExW, LOGFONTW, TEXTMETRICW, DEFAULT_CHARSET,
}};

unsafe extern "system" fn collect(font:*const LOGFONTW,_metrics:*const TEXTMETRICW,_kind:u32,data:LPARAM)->i32 {
    if font.is_null(){return 1;}
    let face=&(*font).lfFaceName;
    let end=face.iter().position(|c|*c==0).unwrap_or(face.len());
    let name=String::from_utf16_lossy(&face[..end]);
    // @ variants are vertical writing aliases, not separate choices for this UI.
    if !name.is_empty()&&!name.starts_with('@') {
        let names=&mut *(data.0 as *mut BTreeMap<String,String>);
        names.entry(name.to_lowercase()).or_insert(name);
    }
    1
}

pub fn installed()->Result<Vec<String>,String>{unsafe{
    let dc=GetDC(None);
    if dc.is_invalid(){return Err("无法读取系统字体".into());}
    let mut families=BTreeMap::<String,String>::new();
    let mut query=LOGFONTW::default();query.lfCharSet=DEFAULT_CHARSET;
    EnumFontFamiliesExW(dc,&query,Some(collect),LPARAM(&mut families as *mut _ as isize),0);
    let _=ReleaseDC(None,dc);
    if families.is_empty(){return Err("系统未返回字体列表".into());}
    Ok(families.into_values().collect())
}}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enumerates_actual_families_without_vertical_aliases_or_duplicates(){
        let names=installed().unwrap();
        assert!(!names.is_empty());
        let keys:std::collections::HashSet<_>=names.iter().map(|s|s.to_lowercase()).collect();
        assert_eq!(keys.len(),names.len());assert!(names.iter().all(|n|!n.starts_with('@')&&n.encode_utf16().count()<=31));
        eprintln!("Installed font families: {}",names.len());
    }
}
