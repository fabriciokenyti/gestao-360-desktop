//! Gestão 360 no PC — casca desktop (spec 027, entrega C).
//!
//! A casca é FINA: carrega o site do escritório (o endereço informado no primeiro
//! uso) em duas janelas e só acrescenta o que um navegador não faz:
//! - **entalho**: janela FIXA (do tamanho do painel), sem borda, transparente,
//!   sempre no topo, centralizada no alto do monitor. A pílula (190×36) vira o
//!   painel (880×500) por animação CSS — a janela não muda de tamanho, então a
//!   animação é a mesma do navegador. A área vazia DEIXA O CLIQUE PASSAR para o
//!   programa de baixo: um vigia leve (`vigiar_mouse`) lê a posição do mouse e só
//!   liga a interação quando ele está sobre o desenho (pílula ou painel). A página
//!   diz qual desenho vale (`entalho_aberto`).
//! - **principal**: o Gestão 360 completo, SOB DEMANDA: nasce ao abrir (bandeja,
//!   "Abrir app", Entrar) e fechar a destrói, liberando a memória (0.2.1).
//! - **bandeja**: mostrar/ocultar o entalho, abrir o sistema, iniciar com o
//!   Windows, trocar o endereço, atualizar, sair.
//! - **atualização**: confere ao abrir e a cada 6 h; avisa e espera o clique na
//!   bandeja (nunca atualiza sozinho). Só aceita versão ASSINADA pela chave do
//!   escritório (chave pública em tauri.conf.json).
//!
//! O site remoto só pode chamar SEIS comandos (ver `liberar_site`). O app não
//! age sobre o PC (decisão de 15/11): nada de abrir programas, arquivos, volume.

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{
    ipc::CapabilityBuilder,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, PhysicalPosition, PhysicalSize, RunEvent, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

const ENTALHO: &str = "entalho";
const PRINCIPAL: &str = "principal";
const CONFIGURACAO: &str = "configuracao";

/// Desenho inicial (a pílula de "Entrar", 190×36) até a página informar o dela.
/// Depois vale o que a página manda em `entalho_aberto` (96×36 em repouso,
/// 380×36 com aviso, 880×500 aberto — ESPELHO do CSS de Entalho.tsx).
const PILULA: (f64, f64) = (190.0, 36.0);
/// Janela fixa: o painel + folga para a sombra.
const JANELA: (f64, f64) = (912.0, 532.0);
/// Folga em volta da pílula para o mouse "pegar" (fica mais fácil de achar).
const FOLGA_PILULA: f64 = 6.0;
/// O vigia lê o mouse a cada 70 ms (~14×/s). GetCursorPos é barato: o custo é
/// desprezível e só existe com o entalho visível.
const VIGIA_MS: u64 = 70;

/// Tamanho (px CSS) do desenho que vale agora; a página avisa em `entalho_aberto`.
static FORMA_L: AtomicU32 = AtomicU32::new(PILULA.0 as u32);
static FORMA_A: AtomicU32 = AtomicU32::new(PILULA.1 as u32);
static VIGIA_LIGADO: AtomicBool = AtomicBool::new(false);
/// Esconder o entalho quando um programa está em tela cheia (preferência na bandeja).
static ESCONDER_TELA_CHEIA: AtomicBool = AtomicBool::new(true);
/// O usuário escondeu de propósito (Ocultar / bandeja): a tela cheia não traz de volta.
static OCULTO_PELO_USUARIO: AtomicBool = AtomicBool::new(false);
/// Quem escondeu foi a tela cheia: ao sair dela, o entalho volta.
static OCULTO_POR_TELA_CHEIA: AtomicBool = AtomicBool::new(false);
/// A tela cheia é conferida a cada N voltas do vigia (~0,5 s).
const TELA_CHEIA_A_CADA: u32 = 7;
/// A cada quantas voltas do vigia (~2 s) confere se a janela continua no meio.
const RECENTRAR_A_CADA: u32 = 30;

/// Atualização: 1ª conferência pouco depois de abrir (sem disputar a partida do
/// Windows), depois a cada 6 h. Uma consulta pequena ao GitHub; nada mais.
const PRIMEIRA_CONFERENCIA: Duration = Duration::from_secs(20);
const CONFERIR_A_CADA: Duration = Duration::from_secs(6 * 60 * 60);
/// Versão nova já encontrada (e avisada): o item da bandeja passa a instalar.
static VERSAO_NOVA: Mutex<Option<String>> = Mutex::new(None);
/// Evita dois downloads se o item for clicado duas vezes.
static INSTALANDO: AtomicBool = AtomicBool::new(false);
/// Abriu agora depois de atualizar: de qual versão veio (vale para esta execução).
static ATUALIZADO_DE: Mutex<Option<String>> = Mutex::new(None);

/// Item "Procurar atualizações" / "Atualizar para a versão X" da bandeja.
struct ItemAtualizar(MenuItem<tauri::Wry>);

// ── configuração (endereço do Gestão 360 do escritório) ──────────────────────

#[derive(Default, Serialize, Deserialize)]
struct Config {
    endereco: Option<String>,
    /// None = padrão (esconde).
    esconder_tela_cheia: Option<bool>,
    /// Versão em uso quando começou uma atualização: ao reabrir, se mudou, o app
    /// avisa "Atualizado para a versão X" (entalho e janela principal).
    versao_anterior: Option<String>,
}

fn caminho_config(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("config.json"))
}

fn ler_config(app: &AppHandle) -> Config {
    caminho_config(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn gravar_config(app: &AppHandle, c: &Config) -> Result<(), String> {
    let p = caminho_config(app).ok_or("sem pasta de configuração")?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    fs::write(p, json).map_err(|e| e.to_string())
}

/// Aceita só https (ou http em localhost, para desenvolvimento) e guarda a ORIGEM.
fn endereco_valido(s: &str) -> Result<Url, String> {
    let u = Url::parse(s.trim())
        .map_err(|_| "Endereço inválido. Exemplo: https://gestao.seuescritorio.com.br".to_string())?;
    let local = matches!(u.host_str(), Some("localhost") | Some("127.0.0.1"));
    match u.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return Err("Use um endereço que comece com https://".into()),
    }
    if u.host_str().is_none() {
        return Err("Endereço sem domínio.".into());
    }
    Url::parse(&u.origin().ascii_serialization()).map_err(|e| e.to_string())
}

fn base_configurada(app: &AppHandle) -> Option<Url> {
    ler_config(app).endereco.as_deref().and_then(|e| endereco_valido(e).ok())
}

// ── permissões do site remoto ────────────────────────────────────────────────

/// O site do escritório (e só ele) pode chamar estes seis comandos, nas duas
/// janelas da casca. Nada mais nativo fica exposto à página remota.
fn liberar_site(app: &AppHandle, base: &Url) {
    let origem = base.origin().ascii_serialization();
    let id = format!("site-{}", base.host_str().unwrap_or("gestao360").replace('.', "-"));
    let resultado = app.add_capability(
        CapabilityBuilder::new(id)
            .remote(format!("{origem}/*"))
            .window(ENTALHO)
            .window(PRINCIPAL)
            .permission("allow-entalho-aberto")
            .permission("allow-entalho-ocultar")
            .permission("allow-abrir-principal")
            .permission("allow-notificar")
            .permission("allow-estado-app")
            .permission("allow-atualizar-app"),
    );
    if let Err(e) = resultado {
        // já liberado nesta execução (trocou e voltou ao mesmo endereço): segue
        eprintln!("[casca] capability: {e}");
    }
}

// ── janelas ──────────────────────────────────────────────────────────────────

/// Monitor em que a janela está (ou o principal, se o Windows não disser).
fn monitor_da(w: &WebviewWindow) -> Option<tauri::Monitor> {
    match w.current_monitor().ok().flatten() {
        Some(m) => Some(m),
        None => w.primary_monitor().ok().flatten(),
    }
}

/// Onde a janela do entalho deve ficar neste monitor: centralizada no alto,
/// no tamanho da janela convertido pela escala do monitor.
fn lugar_no_monitor(m: &tauri::Monitor) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let escala = m.scale_factor();
    let area = *m.size();
    let largura = ((JANELA.0 * escala) as u32).min(area.width);
    let altura = ((JANELA.1 * escala) as u32).min(area.height);
    let x = m.position().x + (area.width as i32 - largura as i32) / 2;
    (PhysicalPosition::new(x, m.position().y), PhysicalSize::new(largura, altura))
}

/// Janela FIXA do entalho, centralizada no alto do monitor em que ela está.
fn posicionar(w: &WebviewWindow) -> tauri::Result<()> {
    let Some(m) = monitor_da(w) else { return Ok(()) };
    let (pos, tam) = lugar_no_monitor(&m);
    w.set_size(tam)?;
    w.set_position(pos)?;
    Ok(())
}

/// A janela saiu do lugar? Acontece quando a tela muda com o app aberto: outra
/// resolução, outra escala do Windows, monitor plugado/tirado, notebook na base.
/// O Windows não reposiciona a janela por nós, e a pílula ficava fora do meio.
fn fora_do_lugar(w: &WebviewWindow) -> bool {
    let Some(m) = monitor_da(w) else { return false };
    let (alvo, tam_alvo) = lugar_no_monitor(&m);
    let (Ok(pos), Ok(tam)) = (w.outer_position(), w.outer_size()) else { return false };
    (pos.x - alvo.x).abs() > 2
        || (pos.y - alvo.y).abs() > 2
        || tam.width.abs_diff(tam_alvo.width) > 2
        || tam.height.abs_diff(tam_alvo.height) > 2
}

/// O mouse está sobre o desenho que vale agora (pílula ou painel)?
/// O desenho fica centralizado no alto da janela, como no CSS.
fn mouse_no_desenho(w: &WebviewWindow) -> Option<bool> {
    let mouse = w.app_handle().cursor_position().ok()?;
    let pos = w.outer_position().ok()?;
    let tam = w.outer_size().ok()?;
    let escala = w.scale_factor().ok()?;
    let lw = FORMA_L.load(Ordering::Relaxed) as f64;
    let lh = FORMA_A.load(Ordering::Relaxed) as f64;
    // pílula (baixinha) ganha folga para o mouse achar; o painel não precisa
    let folga = if lh < 100.0 { FOLGA_PILULA } else { 0.0 };
    let largura = (lw + 2.0 * folga) * escala;
    let altura = (lh + folga) * escala;
    let x0 = pos.x as f64 + (tam.width as f64 - largura) / 2.0;
    let y0 = pos.y as f64;
    Some(mouse.x >= x0 && mouse.x <= x0 + largura && mouse.y >= y0 && mouse.y <= y0 + altura)
}

/// Vigia do mouse: fora do desenho, a janela deixa o clique passar para o
/// programa de baixo (Revit etc.); dentro, a página recebe o mouse normalmente.
/// Ao sair, avisa a página (`g360:mouse-fora`), que fecha com o atraso de sempre.
fn vigiar_mouse(app: AppHandle) {
    if VIGIA_LIGADO.swap(true, Ordering::SeqCst) {
        return; // já há um vigia
    }
    thread::spawn(move || {
        let mut dentro_antes: Option<bool> = None;
        let mut voltas: u32 = 0;
        loop {
            thread::sleep(Duration::from_millis(VIGIA_MS));
            let Some(w) = app.get_webview_window(ENTALHO) else {
                dentro_antes = None;
                continue;
            };
            // Programa em tela cheia (apresentação, vídeo, jogo)? O entalho sai da
            // frente e volta sozinho depois. Revit MAXIMIZADO não conta: não cobre
            // a barra de tarefas.
            voltas = voltas.wrapping_add(1);
            if voltas % TELA_CHEIA_A_CADA == 0 {
                let cheia = ESCONDER_TELA_CHEIA.load(Ordering::Relaxed) && tela_cheia_em_primeiro_plano();
                let visivel = w.is_visible().unwrap_or(false);
                if cheia && visivel {
                    let _ = w.hide();
                    OCULTO_POR_TELA_CHEIA.store(true, Ordering::Relaxed);
                } else if !cheia
                    && OCULTO_POR_TELA_CHEIA.load(Ordering::Relaxed)
                    && !OCULTO_PELO_USUARIO.load(Ordering::Relaxed)
                {
                    let _ = w.show();
                    OCULTO_POR_TELA_CHEIA.store(false, Ordering::Relaxed);
                }
            }
            if !w.is_visible().unwrap_or(false) {
                continue;
            }
            // A tela mudou com o app aberto? Volta para o meio do alto do monitor.
            if voltas % RECENTRAR_A_CADA == 0 && fora_do_lugar(&w) {
                let _ = posicionar(&w);
            }
            let Some(dentro) = mouse_no_desenho(&w) else { continue };
            if dentro_antes != Some(dentro) {
                let _ = w.set_ignore_cursor_events(!dentro);
                if dentro_antes == Some(true) && !dentro {
                    let _ = w.eval("window.dispatchEvent(new Event('g360:mouse-fora'))");
                }
                dentro_antes = Some(dentro);
            }
        }
    });
}

/// A janela em primeiro plano (de OUTRO programa) cobre o monitor inteiro?
/// Só leitura: GetForegroundWindow + retângulo da janela × do monitor.
#[cfg(windows)]
fn tela_cheia_em_primeiro_plano() -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    };
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == std::process::id() {
            return false; // a própria janela do Gestão 360
        }
        // a área de trabalho (ícones) também "cobre o monitor": não é tela cheia
        let mut classe = [0u16; 64];
        let n = GetClassNameW(h, classe.as_mut_ptr(), classe.len() as i32);
        let nome = String::from_utf16_lossy(&classe[..n.max(0) as usize]);
        if nome == "Progman" || nome == "WorkerW" {
            return false;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(h, &mut r) == 0 {
            return false;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut info) == 0 {
            return false;
        }
        let m = info.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

#[cfg(not(windows))]
fn tela_cheia_em_primeiro_plano() -> bool {
    false
}

/// Ctrl+Alt+G: mostra o entalho (se estava escondido) e abre/fecha o painel.
fn atalho_entalho(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(ENTALHO) {
        OCULTO_PELO_USUARIO.store(false, Ordering::Relaxed);
        if !w.is_visible().unwrap_or(false) {
            let _ = posicionar(&w);
            let _ = w.show();
        }
        let _ = w.set_focus();
        let _ = w.eval("window.dispatchEvent(new Event('g360:alternar'))");
    }
}

/// Janela principal SOB DEMANDA (0.2.1): só existe enquanto está aberta. Fechar
/// a destrói de verdade — escondida, ela segurava ~150 MB à toa (medido em 17/11:
/// 353 MB com ela × meta de 250 MB da spec 027). O entalho segue sempre vivo.
fn criar_principal(app: &AppHandle, caminho: &str) -> Result<(), String> {
    let base = base_configurada(app).ok_or("sem endereço configurado")?;
    let url = base.join(caminho).map_err(|e| e.to_string())?;
    if url.origin() != base.origin() {
        return Err("fora do site".into());
    }
    WebviewWindowBuilder::new(app, PRINCIPAL, WebviewUrl::External(url))
        .title("Gestão 360")
        .inner_size(1280.0, 800.0)
        .min_inner_size(400.0, 500.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Traz a principal (bandeja, abrir de novo pelo menu Iniciar). Não existe? Cria
/// — fora da thread do evento, que no Windows não pode criar janela e esperar.
fn mostrar_principal(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PRINCIPAL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = criar_principal(&app, "/dashboard") {
            eprintln!("[casca] janela principal: {e}");
        }
    });
}

fn abrir_janelas(app: &AppHandle, base: &Url, oculto: bool) -> tauri::Result<()> {
    liberar_site(app, base);

    // iniciado pelo Windows (--oculto): só o entalho; a principal nasce no clique
    if !oculto && app.get_webview_window(PRINCIPAL).is_none() {
        let url = base.join("/dashboard").expect("caminho fixo válido");
        WebviewWindowBuilder::new(app, PRINCIPAL, WebviewUrl::External(url))
            .title("Gestão 360")
            .inner_size(1280.0, 800.0)
            .min_inner_size(400.0, 500.0)
            .build()?;
    }

    if app.get_webview_window(ENTALHO).is_none() {
        let url = base.join("/desktop/entalho").expect("caminho fixo válido");
        let w = WebviewWindowBuilder::new(app, ENTALHO, WebviewUrl::External(url))
            .title("Gestão 360 · entalho")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focused(false)
            .inner_size(JANELA.0, JANELA.1)
            .build()?;
        posicionar(&w)?;
        // nasce deixando o clique passar; o vigia liga a interação sobre a pílula
        w.set_ignore_cursor_events(true)?;
        FORMA_L.store(PILULA.0 as u32, Ordering::Relaxed);
        FORMA_A.store(PILULA.1 as u32, Ordering::Relaxed);
        vigiar_mouse(app.clone());
    }
    Ok(())
}

fn abrir_configuracao(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(CONFIGURACAO) {
        w.show()?;
        w.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, CONFIGURACAO, WebviewUrl::App("index.html".into()))
        .title("Gestão 360 · endereço")
        .inner_size(480.0, 380.0)
        .resizable(false)
        .center()
        .build()?;
    Ok(())
}

fn alternar_entalho(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(ENTALHO) {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
            OCULTO_PELO_USUARIO.store(true, Ordering::Relaxed);
        } else {
            let _ = posicionar(&w);
            let _ = w.show();
            OCULTO_PELO_USUARIO.store(false, Ordering::Relaxed);
        }
    }
}

// ── comandos (lista fechada; ver build.rs) ───────────────────────────────────
//
// TODOS `async` de propósito. Comando síncrono roda na thread principal; no
// Windows, criar janela (ou perguntar monitor/tamanho, que espera a thread
// principal) dali TRAVA o app — foi o "Conectando…" eterno da 1ª versão.

/// A página informa o desenho que vale (pílula mínima, com aviso, ou painel).
#[tauri::command]
async fn entalho_aberto(aberto: bool, largura: f64, altura: f64) -> Result<(), String> {
    // A janela não muda de tamanho (a animação é CSS): só troca o desenho que o
    // vigia usa para decidir onde o mouse interage. Limitado ao tamanho da janela.
    let _ = aberto; // informativo; o que vale é o tamanho
    let l = largura.clamp(40.0, JANELA.0) as u32;
    let a = altura.clamp(20.0, JANELA.1) as u32;
    FORMA_L.store(l, Ordering::Relaxed);
    FORMA_A.store(a, Ordering::Relaxed);
    Ok(())
}

/// "Ocultar" do entalho: esconde a janela; a bandeja traz de volta.
#[tauri::command]
async fn entalho_ocultar(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(ENTALHO) {
        w.hide().map_err(|e| e.to_string())?;
        OCULTO_PELO_USUARIO.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Notificação do Windows (lembrete "às 15h"). Textos curtos, só texto.
#[tauri::command]
async fn notificar(app: AppHandle, titulo: String, corpo: String) -> Result<(), String> {
    aviso(&app, &titulo, &corpo)
}

fn aviso(app: &AppHandle, titulo: &str, corpo: &str) -> Result<(), String> {
    let corta = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    app.notification()
        .builder()
        .title(corta(titulo, 80))
        .body(corta(corpo, 200))
        .show()
        .map_err(|e| e.to_string())
}

// ── atualização do programa ──────────────────────────────────────────────────
//
// 0.3.0: à vista, não só na bandeja (18/11, padrão do Kobi). A casca conta às
// páginas (entalho e principal) pelo evento "g360:app" e responde `estado_app`;
// elas mostram "Nova versão · clique", a barrinha "Atualizando 62%" e, ao
// reabrir, "Atualizado para a versão X".

/// Manda um evento "g360:app" para as janelas do site. `detalhe` é JSON montado aqui.
fn avisar_paginas(app: &AppHandle, detalhe: &serde_json::Value) {
    let js = format!("window.dispatchEvent(new CustomEvent('g360:app',{{detail:{detalhe}}}))");
    for rotulo in [ENTALHO, PRINCIPAL] {
        if let Some(w) = app.get_webview_window(rotulo) {
            let _ = w.eval(&js);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EstadoApp {
    versao: String,
    nova: Option<String>,
    atualizado_de: Option<String>,
}

/// O site pergunta: versão do programa, se há nova e se acabou de atualizar.
#[tauri::command]
async fn estado_app(app: AppHandle) -> EstadoApp {
    EstadoApp {
        versao: app.package_info().version.to_string(),
        nova: VERSAO_NOVA.lock().ok().and_then(|v| v.clone()),
        atualizado_de: ATUALIZADO_DE.lock().ok().and_then(|v| v.clone()),
    }
}

/// O site pede para atualizar (aviso do entalho, botão da janela principal). Só
/// instala pacote ASSINADO pela chave do escritório; o resto é igual à bandeja.
#[tauri::command]
async fn atualizar_app(app: AppHandle) -> Result<(), String> {
    if INSTALANDO.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    if let Err(e) = instalar_atualizacao(&app).await {
        avisar_paginas(&app, &serde_json::json!({ "tipo": "erro", "mensagem": e }));
        let _ = aviso(&app, "Não deu para atualizar agora", &format!("Tente de novo mais tarde. ({e})"));
    }
    INSTALANDO.store(false, Ordering::SeqCst);
    Ok(())
}

/// Ao abrir: se a versão mudou desde o início da última atualização, guarda
/// "atualizado de X" para as páginas e limpa a marca.
fn registrar_atualizacao_concluida(app: &AppHandle) {
    let mut config = ler_config(app);
    let Some(antes) = config.versao_anterior.take() else { return };
    let agora = app.package_info().version.to_string();
    if antes != agora {
        if let Ok(mut v) = ATUALIZADO_DE.lock() {
            *v = Some(antes);
        }
    }
    let _ = gravar_config(app, &config);
}
//
// Só a CASCA se atualiza por aqui (o sistema em si é o site: atualiza sozinho).
// O pacote vem dos releases do repositório público e só é aceito se a
// assinatura bater com a chave pública de tauri.conf.json.

/// Confere se há versão nova. Achou: troca o item da bandeja e avisa (uma vez
/// por versão; de novo se foi o usuário que pediu). `pedido` = clique na bandeja.
async fn conferir_atualizacao(app: &AppHandle, pedido: bool) -> Result<(), String> {
    let nova = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(nova) = nova else {
        if pedido {
            let atual = app.package_info().version.to_string();
            aviso(app, "O Gestão 360 está em dia", &format!("Você já está na versão mais nova ({atual})."))?;
        }
        return Ok(());
    };
    let versao = nova.version.clone();
    avisar_paginas(app, &serde_json::json!({ "tipo": "nova", "versao": versao }));
    if let Some(item) = app.try_state::<ItemAtualizar>() {
        let _ = item.0.set_text(format!("Atualizar para a versão {versao}"));
    }
    let ja_avisada = {
        let mut guardada = VERSAO_NOVA.lock().map_err(|e| e.to_string())?;
        let igual = guardada.as_deref() == Some(versao.as_str());
        *guardada = Some(versao.clone());
        igual
    };
    if pedido || !ja_avisada {
        aviso(
            app,
            &format!("Nova versão do Gestão 360 ({versao})"),
            "Quando quiser: clique no aviso do entalho (ou em Atualizar na bandeja). Leva segundos.",
        )?;
    }
    Ok(())
}

/// Baixa, confere a assinatura e instala. No Windows o instalador fecha o app e
/// o abre de novo sozinho no fim.
async fn instalar_atualizacao(app: &AppHandle) -> Result<(), String> {
    let nova = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(nova) = nova else {
        return conferir_atualizacao(app, true).await;
    };
    // marca a versão de agora: ao reabrir, se mudou, o app avisa "Atualizado"
    let mut config = ler_config(app);
    config.versao_anterior = Some(app.package_info().version.to_string());
    let _ = gravar_config(app, &config);

    avisar_paginas(app, &serde_json::json!({ "tipo": "progresso", "pct": 0 }));
    let mut baixado: u64 = 0;
    let mut ultimo: i64 = -1;
    let app_progresso = app.clone();
    let app_fim = app.clone();
    nova
        .download_and_install(
            move |pedaco, total| {
                baixado += pedaco as u64;
                // porcentagem só quando muda (no máx. ~100 eventos)
                let pct = match total {
                    Some(t) if t > 0 => ((baixado * 100) / t).min(100) as i64,
                    _ => 0,
                };
                if pct != ultimo {
                    ultimo = pct;
                    avisar_paginas(&app_progresso, &serde_json::json!({ "tipo": "progresso", "pct": pct }));
                }
            },
            move || avisar_paginas(&app_fim, &serde_json::json!({ "tipo": "instalando" })),
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

/// Clique no item da bandeja: com versão nova já encontrada, instala; senão, procura.
fn atualizar_pela_bandeja(app: &AppHandle) {
    if INSTALANDO.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pronta = VERSAO_NOVA.lock().map(|v| v.is_some()).unwrap_or(false);
        let resultado = if pronta {
            instalar_atualizacao(&app).await
        } else {
            conferir_atualizacao(&app, true).await
        };
        if let Err(e) = resultado {
            avisar_paginas(&app, &serde_json::json!({ "tipo": "erro", "mensagem": e }));
            let _ = aviso(&app, "Não deu para atualizar agora", &format!("Tente de novo mais tarde. ({e})"));
        }
        INSTALANDO.store(false, Ordering::SeqCst);
    });
}

/// Confere sozinho ao abrir (após 20 s) e a cada 6 h. Só AVISA; nunca instala.
fn vigiar_atualizacoes(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(PRIMEIRA_CONFERENCIA);
        loop {
            if let Err(e) = tauri::async_runtime::block_on(conferir_atualizacao(&app, false)) {
                eprintln!("[casca] atualização: {e}"); // sem internet etc.: tenta na próxima
            }
            thread::sleep(CONFERIR_A_CADA);
        }
    });
}

/// Abre um caminho DO PRÓPRIO SITE na janela principal ("Abrir app", Entrar…).
#[tauri::command]
async fn abrir_principal(app: AppHandle, caminho: String) -> Result<(), String> {
    if !caminho.starts_with('/') || caminho.starts_with("//") || caminho.contains('\\') {
        return Err("caminho inválido".into());
    }
    let base = base_configurada(&app).ok_or("sem endereço configurado")?;
    let alvo = base.join(&caminho).map_err(|e| e.to_string())?;
    if alvo.origin() != base.origin() {
        return Err("fora do site".into());
    }
    // principal fechada (agora ela é destruída ao fechar): nasce já no caminho pedido
    let Some(w) = app.get_webview_window(PRINCIPAL) else {
        return criar_principal(&app, &caminho);
    };
    let destino = serde_json::to_string(alvo.as_str()).map_err(|e| e.to_string())?;
    w.eval(&format!("window.location.assign({destino})")).map_err(|e| e.to_string())?;
    mostrar_principal(&app);
    Ok(())
}

/// Tela local de primeiro uso (ou "Trocar endereço…" na bandeja).
#[tauri::command]
async fn definir_endereco(app: AppHandle, endereco: String) -> Result<(), String> {
    let base = endereco_valido(&endereco)?;
    let mut config = ler_config(&app);
    config.endereco = Some(base.to_string());
    gravar_config(&app, &config)?;
    for rotulo in [ENTALHO, PRINCIPAL] {
        if let Some(w) = app.get_webview_window(rotulo) {
            let _ = w.destroy();
        }
    }
    abrir_janelas(&app, &base, false).map_err(|e| e.to_string())?;
    if let Some(w) = app.get_webview_window(CONFIGURACAO) {
        let _ = w.destroy();
    }
    Ok(())
}

#[tauri::command]
async fn endereco_atual(app: AppHandle) -> Option<String> {
    ler_config(&app).endereco
}

// ── bandeja ──────────────────────────────────────────────────────────────────

fn montar_bandeja(app: &AppHandle) -> tauri::Result<()> {
    let mostrar = MenuItem::with_id(app, "mostrar", "Mostrar / ocultar o entalho", true, None::<&str>)?;
    let abrir = MenuItem::with_id(app, "abrir", "Abrir o Gestão 360", true, None::<&str>)?;
    let ligado = app.autolaunch().is_enabled().unwrap_or(false);
    let iniciar = CheckMenuItem::with_id(app, "iniciar", "Iniciar com o Windows", true, ligado, None::<&str>)?;
    let tela_cheia = CheckMenuItem::with_id(
        app,
        "tela-cheia",
        "Esconder com programa em tela cheia",
        true,
        ESCONDER_TELA_CHEIA.load(Ordering::Relaxed),
        None::<&str>,
    )?;
    let endereco = MenuItem::with_id(app, "endereco", "Trocar endereço…", true, None::<&str>)?;
    let atualizar = MenuItem::with_id(app, "atualizar", "Procurar atualizações", true, None::<&str>)?;
    let sair = MenuItem::with_id(app, "sair", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&mostrar, &abrir, &iniciar, &tela_cheia, &endereco, &atualizar, &sair],
    )?;
    app.manage(ItemAtualizar(atualizar.clone()));

    let iniciar_item = iniciar.clone();
    let tela_cheia_item = tela_cheia.clone();
    TrayIconBuilder::new()
        .icon(app.default_window_icon().expect("ícone do app").clone())
        .tooltip(format!("Gestão 360 · versão {}", app.package_info().version))
        .menu(&menu)
        .on_menu_event(move |app, evento| match evento.id.as_ref() {
            "mostrar" => alternar_entalho(app),
            "abrir" => mostrar_principal(app),
            "iniciar" => {
                let quer = iniciar_item.is_checked().unwrap_or(false);
                let autostart = app.autolaunch();
                let _ = if quer { autostart.enable() } else { autostart.disable() };
            }
            "tela-cheia" => {
                let quer = tela_cheia_item.is_checked().unwrap_or(true);
                ESCONDER_TELA_CHEIA.store(quer, Ordering::Relaxed);
                let mut config = ler_config(app);
                config.esconder_tela_cheia = Some(quer);
                let _ = gravar_config(app, &config);
            }
            "endereco" => {
                let _ = abrir_configuracao(app);
            }
            "atualizar" => atualizar_pela_bandeja(app),
            "sair" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

// ── entrada ──────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // abrir de novo (atalho, menu Iniciar) só traz a janela principal
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| mostrar_principal(app)))
        // iniciado pelo Windows → abre só o entalho (a principal fica escondida)
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--oculto"])))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _atalho, evento| {
                    if evento.state() == ShortcutState::Pressed {
                        atalho_entalho(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            entalho_aberto,
            entalho_ocultar,
            abrir_principal,
            notificar,
            estado_app,
            atualizar_app,
            definir_endereco,
            endereco_atual
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            ESCONDER_TELA_CHEIA.store(
                ler_config(&handle).esconder_tela_cheia.unwrap_or(true),
                Ordering::Relaxed,
            );
            montar_bandeja(&handle)?;
            registrar_atualizacao_concluida(&handle);
            vigiar_atualizacoes(handle.clone());
            // Ctrl+Alt+G abre/fecha o entalho de qualquer lugar. Se outro programa
            // já usa o atalho, segue sem ele (não derruba o app).
            let atalho = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyG);
            if let Err(e) = handle.global_shortcut().register(atalho) {
                eprintln!("[casca] atalho Ctrl+Alt+G indisponível: {e}");
            }
            let oculto = std::env::args().any(|a| a == "--oculto");
            match base_configurada(&handle) {
                Some(base) => abrir_janelas(&handle, &base, oculto)?,
                None => abrir_configuracao(&handle)?,
            }
            Ok(())
        })
        .on_window_event(|janela, evento| {
            // fechar o entalho só esconde (a bandeja traz de volta). A principal
            // FECHA de verdade e libera a memória; reabre pela bandeja/entalho.
            if let WindowEvent::CloseRequested { api, .. } = evento {
                if janela.label() == ENTALHO {
                    api.prevent_close();
                    let _ = janela.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o Gestão 360")
        .run(|_app, evento| {
            // fechar a última janela não encerra; só "Sair" na bandeja encerra
            if let RunEvent::ExitRequested { api, code, .. } = evento {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
