fn main() {
    // Lista FECHADA de comandos que a casca expõe. Cada um vira uma permissão
    // `allow-<comando>`; quem pode chamar o quê está nas capabilities:
    // - o SITE (remoto) só recebe os seis primeiros (lib.rs, liberar_site);
    // - a tela local de primeiro uso recebe os dois de endereço
    //   (capabilities/configuracao.json).
    // O app NÃO age sobre o PC (decisão de 15/11): nada de abrir programas,
    // arquivos, volume ou desligar.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "entalho_aberto",
            "entalho_ocultar",
            "abrir_principal",
            "notificar",
            "estado_app",
            "atualizar_app",
            "definir_endereco",
            "endereco_atual",
        ]),
    ))
    .expect("falha no build da casca");
}
