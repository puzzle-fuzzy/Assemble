//! 纯文本仅在具名命令或远端的显式请求中读取，内容不进入 UI、日志或持久化状态。
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::remote::{
    ClipboardOperation, ClipboardResult, ClipboardStatus, ClipboardTask, valid_clipboard,
};

pub struct LocalClipboard(arboard::Clipboard);
impl LocalClipboard {
    pub fn open() -> Result<Self, ClipboardStatus> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|_| ClipboardStatus::Unavailable)
    }
    pub fn read(&mut self) -> Result<String, ClipboardStatus> {
        let text = self
            .0
            .get_text()
            .map_err(|_| ClipboardStatus::Unavailable)?;
        if valid_clipboard(&text) {
            Ok(text)
        } else {
            Err(ClipboardStatus::TooLarge)
        }
    }
    pub fn write(&mut self, text: &str) -> Result<(), ClipboardStatus> {
        if !valid_clipboard(text) {
            return Err(ClipboardStatus::TooLarge);
        }
        self.0
            .set_text(text)
            .map_err(|_| ClipboardStatus::Unavailable)
    }
}
pub(super) fn exchange(task: &ClipboardTask, paste: impl FnOnce() -> bool) -> ClipboardResult {
    let result = if Timestamp::now() >= task.expires_at {
        Err(ClipboardStatus::Expired)
    } else {
        LocalClipboard::open().and_then(|mut clipboard| match &task.request.operation {
            ClipboardOperation::Read => clipboard.read().map(Some),
            ClipboardOperation::Write {
                text,
                paste: requested,
            } => {
                clipboard.write(text)?;
                if *requested && !paste() {
                    return Err(ClipboardStatus::Unavailable);
                }
                Ok(None)
            }
        })
    };
    match result {
        Ok(text) => ClipboardResult {
            id: task.request.id.clone(),
            status: ClipboardStatus::Completed,
            text,
        },
        Err(status) => ClipboardResult {
            id: task.request.id.clone(),
            status,
            text: None,
        },
    }
}
