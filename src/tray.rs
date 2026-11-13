//! StatusNotifierItem tray icon (pure Rust D-Bus, no libappindicator).

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;
use ksni::blocking::{Handle, TrayMethods};

use crate::Cmd;

const SIZE: i32 = 32;

pub struct WaTray {
    pub unread: u32,
    tx: async_channel::Sender<Cmd>,
}

impl WaTray {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.try_send(cmd);
    }
}

impl ksni::Tray for WaTray {
    fn id(&self) -> String {
        "walite".into()
    }
    fn title(&self) -> String {
        match self.unread {
            0 => "WhatsApp".into(),
            n => format!("WhatsApp ({n})"),
        }
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let rgba = draw_rgba(self.unread);
        let argb = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        vec![ksni::Icon { width: SIZE, height: SIZE, data: argb }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.title(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::Toggle);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Show".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Show)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn spawn(tx: async_channel::Sender<Cmd>) -> Option<Handle<WaTray>> {
    match (WaTray { unread: 0, tx }).spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("walite: tray unavailable ({e}); closing the window will quit");
            None
        }
    }
}

/// Green round icon with a white chat bubble; red dot when there are unread messages.
fn draw_rgba(unread: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (SIZE * SIZE * 4) as usize];
    let c = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = (x as f32, y as f32);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let mut px = [0u8; 4];
            if d <= c + 0.5 {
                px = [37, 211, 102, 255]; // WhatsApp-ish green
                // white bubble: circle + small tail at bottom-left
                let bd = ((fx - c).powi(2) + (fy - c + 1.0).powi(2)).sqrt();
                let tail = fx > 7.0 && fx < 13.0 && fy > 19.0 && fy < 25.0 && (fx - 7.0) > (fy - 19.0) * 0.5;
                if bd <= 9.0 || tail {
                    px = [255, 255, 255, 255];
                }
            }
            if unread > 0 {
                let rd = ((fx - 25.0).powi(2) + (fy - 6.0).powi(2)).sqrt();
                if rd <= 6.5 {
                    px = [230, 40, 40, 255];
                }
            }
            let i = ((y * SIZE + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&px);
        }
    }
    buf
}

pub fn app_pixbuf(unread: u32) -> Pixbuf {
    let bytes = glib::Bytes::from_owned(draw_rgba(unread));
    Pixbuf::from_bytes(&bytes, Colorspace::Rgb, true, 8, SIZE, SIZE, SIZE * 4)
}
