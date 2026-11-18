use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::barcodeview::BarcodeView;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

/// A label with a Code 128 barcode of the order number. The table, the subsets and the check symbol are in the SDK (`iyakki_ui_sdk::barcode`),
/// tested against independent encodings: do not write your own. `BarcodeView::ean13(action, "590123412345")` and `::upca(..)` draw EAN-13 and UPC-A.
pub struct LabelApp {
    code: BarcodeView,
}

impl LabelApp {
    pub fn new(order: &str) -> Result<LabelApp, String> {
        Ok(LabelApp { code: BarcodeView::code128(1, order)? })
    }
    pub fn modules(&self) -> usize {
        self.code.modules().len()
    }
}

impl UiApp for LabelApp {
    fn id(&self) -> &'static str { "label-demo" }
    fn title(&self) -> String { "Label demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let UiEvent::Key(UiKey::Esc) = ev { return Flow::Back; }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.code.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, (h - 2.0 * g).min(w / 2.0)));
    }
}
