//! Writes one page of a PDF as an enhanced metafile, for checking the vector print path offline.
//! Usage: cargo run -p pdfcraft-render --example emf_dump -- <in.pdf> <page 1-based> <out.emf>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: emf_dump <in.pdf> <page 1-based> <out.emf>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).expect("read the PDF");
    let page: usize = args[2].parse().expect("page number");
    match pdfcraft_render::emf::page_to_emf(&bytes, page - 1) {
        Ok(out) => {
            std::fs::write(&args[3], &out.bytes).expect("write the metafile");
            println!("{} bytes, {:.2} x {:.2} pt", out.bytes.len(), out.width_pt, out.height_pt);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
