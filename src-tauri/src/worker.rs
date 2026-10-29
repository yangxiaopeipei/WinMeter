use std::sync::{mpsc,Mutex};
pub struct SamplingWorker {stop:mpsc::Sender<()>,thread:Mutex<Option<std::thread::JoinHandle<()>>>}
impl SamplingWorker {
    pub fn spawn(work:impl FnOnce(mpsc::Receiver<()>)+Send+'static)->Self {
        let (stop,receive)=mpsc::channel();
        Self{stop,thread:Mutex::new(Some(std::thread::spawn(move||{
            // 采样循环里的意外只该停掉采样，不该把整个程序带走。
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(||work(receive))).is_err(){
                eprintln!("采样线程内部错误退出，界面保留最后一帧");
            }
        })))}
    }
    pub fn shutdown(&self){
        let _=self.stop.send(());
        if let Some(thread)=self.thread.lock().unwrap_or_else(|p|p.into_inner()).take(){
            if thread.join().is_err(){eprintln!("采样线程异常退出，历史可能未完整保存");}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_wakes_waiter_and_waits_for_final_flush(){
        let flushed=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let copy=flushed.clone();
        let worker=SamplingWorker::spawn(move|stop|{let _=stop.recv_timeout(std::time::Duration::from_secs(60));copy.store(true,std::sync::atomic::Ordering::SeqCst);});
        worker.shutdown();worker.shutdown();
        assert!(flushed.load(std::sync::atomic::Ordering::SeqCst));
    }
}
