//! Observed process I/O, kept separate from physical disk and SMART counters.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct AppDay {
    pub day:i64, pub name:String, pub path:Option<String>, pub read:u64, pub write:u64,
}
#[derive(Default, Deserialize, Serialize)]
pub struct History {#[serde(default)] pub days:Vec<AppDay>}
#[derive(Clone, Default, Serialize)]
pub struct AppTotal {pub name:String,pub path:Option<String>,pub read:u64,pub write:u64}
#[derive(Clone, Default, Serialize)]
pub struct Summary {pub today:Vec<AppTotal>,pub week:Vec<AppTotal>,pub month:Vec<AppTotal>}

fn identity(name:&str,path:Option<&str>)->String {
    match path {Some(p)=>format!("path:{}",p.to_lowercase()),None=>format!("name:{}",name.to_lowercase())}
}
impl History {
    pub fn add(&mut self,day:i64,name:&str,path:Option<&str>,read:u64,write:u64) {
        if read==0&&write==0{return;}
        let key=identity(name,path);
        if let Some(row)=self.days.iter_mut().rev().take_while(|r|r.day==day).find(|r|identity(&r.name,r.path.as_deref())==key) {
            row.read=row.read.saturating_add(read);row.write=row.write.saturating_add(write);
        }else{self.days.push(AppDay{day,name:name.into(),path:path.map(str::to_owned),read,write});}
    }
    pub fn prune(&mut self,today:i64){self.days.retain(|r|r.day>=today-29*86400&&r.day<=today);}
    fn totals(&self,start:i64,today:i64)->Vec<AppTotal>{
        let mut totals:HashMap<String,AppTotal>=HashMap::new();
        for row in self.days.iter().filter(|r|r.day>=start&&r.day<=today) {
            let total=totals.entry(identity(&row.name,row.path.as_deref())).or_insert_with(||AppTotal{name:row.name.clone(),path:row.path.clone(),..Default::default()});
            total.read=total.read.saturating_add(row.read);total.write=total.write.saturating_add(row.write);
        }
        let mut rows:Vec<_>=totals.into_values().collect();
        rows.sort_by(|a,b|b.write.cmp(&a.write).then(b.read.cmp(&a.read)).then(a.name.cmp(&b.name)));
        rows.truncate(20);rows
    }
    pub fn summary(&self,today:i64)->Summary{Summary{today:self.totals(today,today),week:self.totals(today-6*86400,today),month:self.totals(today-29*86400,today)}}
}

/// A reused PID or a missing counter is a new baseline, never historical traffic.
pub fn delta(previous:Option<(u64,u64,u64)>,created:u64,read:u64,write:u64)->Option<(u64,u64)> {
    previous.filter(|p|p.0==created&&read>=p.1&&write>=p.2).map(|p|(read-p.1,write-p.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pid_reuse_missing_samples_and_counter_reset_do_not_create_writes(){
        assert_eq!(delta(None,1,10000,20000),None);
        assert_eq!(delta(Some((1,10,20)),2,10000,20000),None);
        assert_eq!(delta(Some((1,10,20)),1,9,21),None);
        assert_eq!(delta(Some((1,10,20)),1,15,28),Some((5,8)));
    }
    #[test]
    fn apps_group_by_path_across_restarts_and_keep_periods_and_names_separate(){
        let mut h=History::default();let today=30*86400;
        h.add(today,"a.exe",Some("C:\\a.exe"),1,10);h.add(today,"a.exe",Some("c:\\a.exe"),2,20);
        h.add(today,"a.exe",Some("D:\\a.exe"),0,5);
        h.add(today-86400,"a.exe",Some("C:\\a.exe"),0,100);
        h.add(today-8*86400,"b.exe",None,0,200);
        h.add(today-30*86400,"old.exe",None,0,9999);
        h.prune(today);let s=h.summary(today);
        assert_eq!(s.today.len(),2);assert_eq!(s.today[0].write,30);
        assert_eq!(s.week[0].write,130);assert_eq!(s.month[0].name,"b.exe");
        let saved=serde_json::to_string(&h).unwrap();
        let restored:History=serde_json::from_str(&saved).unwrap();assert_eq!(restored.summary(today).month[0].write,200);
    }
}
