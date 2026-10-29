//! STUB
use super::{BarInfo, Ctx, Module};
pub struct Tray;
impl Tray {
    pub fn new(_ctx: &Ctx) -> Tray {
        Tray
    }
}
impl Module for Tray {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        gtk::Box::new(gtk::Orientation::Horizontal, 0).into()
    }
}
