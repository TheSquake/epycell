//! Standalone sixel de-risk test.
//!
//! Loads the PNG captured by the kernel spike (`out-0.png`) and renders it in
//! the terminal via ratatui-image, auto-detecting the graphics protocol
//! (sixel in Foot). Press any key to quit.
//!
//!   cargo run --bin sixel_test

use std::path::PathBuf;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Layout, Size};
use ratatui::widgets::{Block, Borders};
use ratatui_image::{picker::Picker, Image, Resize};

fn main() -> Result<()> {
    let png_path = PathBuf::from(std::env::var("HOME")?).join("epycell/out-0.png");

    // Detect protocol + font size BEFORE taking over the terminal.
    let picker = Picker::from_query_stdio().context("querying terminal graphics support")?;
    let proto_type = picker.protocol_type();

    // Decode the captured figure.
    let dyn_img = image::ImageReader::open(&png_path)
        .with_context(|| format!("opening {} (run `cargo run` first to create it)", png_path.display()))?
        .decode()
        .context("decoding png")?;

    // Size the image in terminal cells from its pixel size and the font size.
    let fs = picker.font_size();
    let size = Size::new(
        (dyn_img.width().div_ceil(fs.width as u32)) as u16,
        (dyn_img.height().div_ceil(fs.height as u32)) as u16,
    );
    let protocol = picker
        .new_protocol(dyn_img, size, Resize::Fit(None))
        .context("encoding image to terminal graphics protocol")?;

    // Take over the terminal and draw.
    let mut terminal = ratatui::init();
    let draw_result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| {
                let [info, img_area] =
                    Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(f.area());
                let label = format!(
                    " epycell sixel test — protocol: {:?} — font {}x{}px — press any key to quit ",
                    proto_type, fs.width, fs.height
                );
                f.render_widget(
                    Block::default().title(label).borders(Borders::BOTTOM),
                    info,
                );
                f.render_widget(Image::new(&protocol), img_area);
            })?;

            if matches!(event::read()?, Event::Key(_)) {
                break;
            }
        }
        Ok(())
    })();

    ratatui::restore();
    draw_result?;
    println!("detected graphics protocol: {:?}", proto_type);
    Ok(())
}
