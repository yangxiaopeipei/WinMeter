//! Read classic/StartAllBack application buttons through MSAA. The enclosing
//! MSTaskList window includes blank space and is not itself an occupied button.
use super::*;
use windows::core::{Interface,IUnknown};

pub(super) unsafe fn descendant(shell:HWND,class:&str)->Option<HWND> {
    struct Search<'a> {class:&'a str,found:Option<HWND>}
    unsafe extern "system" fn visit(hwnd:HWND,lp:LPARAM)->windows::Win32::Foundation::BOOL {
        let search=&mut *(lp.0 as *mut Search<'_>);
        let mut name=[0u16;128]; let n=GetClassNameW(hwnd,&mut name);
        if String::from_utf16_lossy(&name[..n.max(0) as usize])==search.class {
            search.found=Some(hwnd); return windows::Win32::Foundation::BOOL(0);
        }
        windows::Win32::Foundation::BOOL(1)
    }
    let mut search=Search{class,found:None};
    let _=EnumChildWindows(shell,Some(visit),LPARAM(&mut search as *mut _ as isize));
    search.found
}

unsafe fn dispatch(child:&VARIANT)->Option<IAccessible> {
    let raw=&child.as_raw().Anonymous.Anonymous;
    // AccessibleChildren returns VT_DISPATCH objects or VT_I4 child IDs.
    if raw.vt!=9 {return None;}
    let pointer=raw.Anonymous.pdispVal;
    IUnknown::from_raw_borrowed(&pointer)?.cast().ok()
}

unsafe fn item(object:&IAccessible,id:&VARIANT,out:&mut Vec<RECT>) {
    let role=object.get_accRole(id).ok().and_then(|v|u32::try_from(&v).ok());
    if !role.is_some_and(|r|[ROLE_SYSTEM_PUSHBUTTON,ROLE_SYSTEM_LISTITEM,ROLE_SYSTEM_PAGETAB].contains(&r)) {return;}
    // STATE_SYSTEM_INVISIBLE | STATE_SYSTEM_OFFSCREEN, independent of Controls feature bindings.
    let Some(state)=object.get_accState(id).ok().and_then(|v|u32::try_from(&v).ok()) else {return;};
    if state & 0x18000 !=0 {return;}
    let (mut x,mut y,mut width,mut height)=(0,0,0,0);
    if object.accLocation(&mut x,&mut y,&mut width,&mut height,id).is_ok() && width>0 && height>0 {
        out.push(RECT{left:x,top:y,right:x+width,bottom:y+height});
    }
}

unsafe fn walk(object:&IAccessible,depth:u32,budget:&mut usize,out:&mut Vec<RECT>)->Option<()> {
    if depth>8 || *budget==0 {return None;}
    let count=object.accChildCount().ok()?.clamp(0,512) as usize;
    if count==0 {return Some(());}
    if count>*budget {return None;}
    let mut children:Vec<_>=(0..count.min(*budget)).map(|_|VARIANT::default()).collect();
    let mut obtained=0;
    AccessibleChildren(object,0,&mut children,&mut obtained).ok()?;
    if obtained<0 || obtained as usize>children.len() {return None;}
    for child in children.iter().take(obtained as usize) {
        if *budget==0 {return None;}
        *budget-=1;
        let nested=dispatch(child).or_else(||i32::try_from(child).ok()
            .and_then(|_|object.get_accChild(child).ok()).and_then(|d|d.cast::<IAccessible>().ok()));
        if let Some(nested)=nested {
            let before=out.len();
            walk(&nested,depth+1,budget,out)?;
            if out.len()==before {item(&nested,&VARIANT::from(0i32),out);}
        } else {item(object,child,out);}
    }
    Some(())
}

pub(super) unsafe fn buttons(shell:HWND)->Option<(RECT,Vec<RECT>)> {
    let task=descendant(shell,"MSTaskListWClass").or_else(||descendant(shell,"MSTaskSwWClass"))?;
    let mut bounds=RECT::default(); GetWindowRect(task,&mut bounds).ok()?;
    let mut pointer=std::ptr::null_mut();
    AccessibleObjectFromWindow(task,OBJID_CLIENT.0 as u32,&IAccessible::IID,&mut pointer).ok()?;
    if pointer.is_null() {return None;}
    let object=IAccessible::from_raw(pointer);
    let mut rects=Vec::new(); let mut budget=512;
    walk(&object,0,&mut budget,&mut rects)?;
    rects.retain(|r|r.left>=bounds.left && r.right<=bounds.right && r.top<bounds.bottom && r.bottom>bounds.top);
    (!rects.is_empty()).then_some((bounds,rects))
}
