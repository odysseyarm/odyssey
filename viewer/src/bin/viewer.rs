use macroquad::prelude::Conf;

fn window_conf() -> Conf {
    Conf {
        window_title: "donger viewer".into(),
        high_dpi: false, // changing to true didn't change anything
        window_width: 98*10+40,
        window_height: 98*5+80,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    viewer::main().await
}
