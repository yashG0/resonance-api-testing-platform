slint::slint! {
    export component AppWindow inherits Window {
        title: "Resonance";
        width: 1000px;
        height: 680px;
        background: #1e1e2e;

        VerticalLayout {
            alignment: center;
            spacing: 12px;

            Text {
                text: "Resonance";
                color: #cba6f7;
                font-size: 32px;
                font-weight: 700;
                horizontal-alignment: center;
            }

            Text {
                text: "A modern, Linux-first API testing platform.";
                color: #cdd6f4;
                font-size: 16px;
                horizontal-alignment: center;
            }

            Text {
                text: "Built with Rust and Slint.";
                color: #a6adc8;
                font-size: 13px;
                horizontal-alignment: center;
            }
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    app.run()
}
