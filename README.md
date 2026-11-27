# Gestão 360 no PC

Programa de Windows do **Gestão 360**: um entalho no topo da tela (agenda, lembretes,
projetos e financeiro num relance) e o sistema completo numa janela própria.

> Este repositório tem **só o programa (a "casca")**. O Gestão 360 em si é um site, e o código dele
> é privado. Sem um site do Gestão 360 para apontar, este programa não faz nada.
>
> © Engenharia Moderna. **Todos os direitos reservados.** O código está visível para que as
> atualizações possam ser baixadas; isso não concede licença de uso, cópia ou redistribuição.

## Instalar

1. Em [**Releases**](../../releases/latest), baixe o `Gestão 360_X.Y.Z_x64-setup.exe`.
2. Rode o instalador. Ainda **sem assinatura de código**: o Windows mostra "O Windows protegeu o
   computador" → **Mais informações** → **Executar assim mesmo**.
3. No primeiro uso, informe o endereço do Gestão 360 do escritório (o mesmo do navegador).

## Atualizar

O programa confere sozinho ao abrir e a cada 6 horas. Quando há versão nova, **o entalho avisa**
("Nova versão do app · clique para atualizar"), a janela principal mostra um aviso com o botão
**Atualizar**, e o menu da bandeja também oferece. Ele **nunca atualiza sozinho**: você clica quando
quiser, a pílula mostra a barrinha **"Atualizando 62%"**, o app fecha, abre de novo e avisa
**"App atualizado · versão X"**.

Toda versão é **assinada**. O programa só instala o que bater com a chave pública gravada nele
(`src-tauri/tauri.conf.json` → `plugins.updater.pubkey`). Um pacote adulterado é recusado.

## O que o programa faz

| Parte | O que é |
|---|---|
| **Entalho** | Janela fixa, sem borda, transparente, sempre no topo, no alto do monitor. Em repouso é uma pílula mínima; cresce quando há aviso e vira o painel ao passar o mouse. A área vazia deixa o clique passar para o programa de baixo. Se a tela mudar com o app aberto (resolução, escala, monitor), ele volta sozinho para o meio. |
| **Principal** | O Gestão 360 completo. Fechar só esconde: o app vive na bandeja. |
| **Bandeja** | Mostrar/ocultar o entalho · Abrir o Gestão 360 · Iniciar com o Windows · Esconder com programa em tela cheia · Trocar endereço… · Atualizar · Sair. |
| **Atalho** | **Ctrl+Alt+G** abre e fixa o painel de qualquer lugar. |
| **Lembretes** | Na hora de um lembrete, a pílula avisa e sai uma notificação do Windows. |

**Segurança:** o site só pode pedir seis coisas ao programa (`entalho_aberto`, `entalho_ocultar`,
`abrir_principal`, `notificar`, `estado_app` e `atualizar_app` — este só instala versão assinada). O programa **não age sobre o PC**: nada de abrir programas,
arquivos, volume ou desligar. Lista fechada em `src-tauri/build.rs`. Link do site que pede outra aba
(ex.: "abrir em nova aba" da proposta) abre no navegador padrão do Windows (só http/https).

## Publicar uma versão (responsável)

1. Suba a versão em `src-tauri/tauri.conf.json` **e** `src-tauri/Cargo.toml` (ex.: `0.2.0` → `0.3.0`)
   na `main`.
2. **Actions → Publicar versão → Run workflow.** O GitHub compila, assina e cria o release
   `vX.Y.Z` com o instalador e o `latest.json` que os apps instalados consultam.

A assinatura usa o segredo `TAURI_SIGNING_PRIVATE_KEY` deste repositório. A chave privada fica
**fora** do repositório, com cópia de segurança. **Perder a chave = os apps instalados não aceitam
mais atualizações** (seria preciso reinstalar à mão uma versão com chave nova).

Compilar sem publicar: **Actions → Compilar (Windows)** (o instalador fica em *Artifacts*).
