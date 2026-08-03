//! Ventana egui muy simple para disparar la (re)generación de `sde.db`
//! y ver el progreso. Independiente de la app egui principal del proyecto.
//! Requiere la feature `gui` (que a su vez activa `builder`).

fn main() -> eframe::Result<()> {
    eframe::run_simple_native("SDE Builder", eframe::NativeOptions::default(), |ctx, _frame| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("sde-gui: aún no implementado.");
        });
    })
}
