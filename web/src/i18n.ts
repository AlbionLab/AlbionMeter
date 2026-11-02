import { useSyncExternalStore } from 'react';
import { desktop } from './desktop';

export type Language = 'en' | 'pt';

/**
 * The components keep their English text as keys; in Portuguese each one is looked up here. The choice is saved by the
 * desktop app in its settings file (the system's per-user app data folder), or in the browser when the meter runs
 * without it, and the whole window renders again when it changes.
 */
let language: Language = 'en';
const listeners = new Set<() => void>();
const storageKey = 'albion-meter-language';

const fromLocale = (value?: string | null): Language => value?.toLowerCase().startsWith('pt') ? 'pt' : 'en';

function apply(value: Language) {
  language = value;
  document.documentElement.lang = value === 'pt' ? 'pt-BR' : 'en';
  listeners.forEach(listener => listener());
}

/** Reads the saved language before the first render, so the window never opens in one language and switches. */
export async function loadLanguage() {
  let saved: string | null = null;
  try { saved = desktop ? await desktop.language() : localStorage.getItem(storageKey); } catch { saved = null; }
  apply(saved === 'en' || saved === 'pt' ? saved : fromLocale(navigator.language));
}

export function setLanguage(value: Language) {
  if (value === language) return;
  apply(value);
  if (desktop) void desktop.setLanguage(value).catch(() => undefined);
  else try { localStorage.setItem(storageKey, value); } catch { /* the choice lasts until the window closes */ }
}

export const currentLanguage = () => language;
export const locale = () => language === 'pt' ? 'pt-BR' : 'en';
export const useLanguage = () => useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener); }, () => language);

const pt: Record<string, string> = {
  // Top bar
  'Language': 'Idioma',
  'View': 'Visualização',
  'Meter': 'Meter',
  'Capture': 'Captura',
  'Game data': 'Dados do jogo',
  'Albion Meter version': 'Versão do Albion Meter',
  'Reconnecting': 'Reconectando',
  'Recording': 'Gravando',
  'Paused': 'Pausado',
  'Stopped': 'Parado',
  'Ready': 'Pronto',
  'Minimize': 'Minimizar',
  'Maximize': 'Maximizar',
  'Restore': 'Restaurar',
  'Close': 'Fechar',
  'Dismiss': 'Fechar',
  // Meter
  'Encounter': 'Combate',
  'Pause': 'Pausar',
  'Resume': 'Continuar',
  'Play': 'Play',
  'Stop': 'Stop',
  'Reset': 'Zerar',
  'Clear damage and healing and count again from zero, keeping the players and the AlbionLab session': 'Zera o dano e a cura e conta de novo do zero, mantendo os jogadores e a sessão no AlbionLab',
  'Damage': 'Dano',
  'Healing': 'Cura',
  'Metric': 'Métrica',
  '{total} total · {rate} party {unit}': '{total} no total · {rate} {unit} da party',
  'Press Play, then fight: party members appear here.': 'Aperte Play e lute: os membros da party aparecem aqui.',
  'Waiting for combat…': 'Esperando combate…',
  'Loading the meter…': 'Carregando o meter…',
  'The session could not be updated.': 'Não foi possível atualizar a gravação.',
  'The server is unavailable.': 'O serviço de captura não está respondendo.',
  // AlbionLab
  'Connecting to AlbionLab…': 'Conectando ao AlbionLab…',
  'Full analysis, dungeons and sharing at albionlab.app': 'Análise completa, dungeons e compartilhamento em albionlab.app',
  'Connect your AlbionLab account to publish each record as a live session you can open, share or make public.': 'Conecte sua conta do AlbionLab para publicar cada gravação como uma sessão ao vivo que você pode abrir, compartilhar ou deixar pública.',
  'This PC was disconnected from your account. Connect again to keep publishing.': 'Este PC foi desconectado da sua conta. Conecte de novo para continuar publicando.',
  'The connection was refused on the site.': 'A conexão foi recusada no site.',
  'The code expired. Try again.': 'O código expirou. Tente de novo.',
  'Connect account': 'Conectar conta',
  'Approve this PC on AlbionLab (sign in with Discord if asked). The code shown there must be:': 'Aprove este PC no AlbionLab (entre com o Discord se for pedido). O código mostrado lá deve ser:',
  'Open AlbionLab again': 'Abrir o AlbionLab de novo',
  'Waiting for approval…': 'Esperando a aprovação…',
  'Publishing live': 'Publicando ao vivo',
  'Session finished': 'Sessão encerrada',
  'Offline — will resend': 'Offline — vai reenviar',
  'Not published': 'Não publicada',
  'Copied': 'Copiado',
  'Copy link': 'Copiar link',
  'Open full analysis': 'Abrir análise completa',
  'Creating the session…': 'Criando a sessão…',
  'No live session': 'Nenhuma sessão ao vivo',
  'The live session on AlbionLab is being created.': 'A sessão ao vivo no AlbionLab está sendo criada.',
  'Press Play: each record becomes a new live session on AlbionLab, and Stop ends it.': 'Aperte Play: cada gravação vira uma nova sessão ao vivo no AlbionLab, e o Stop encerra.',
  'My sessions': 'Minhas sessões',
  'Visibility': 'Visibilidade',
  'Unlisted — anyone with the link': 'Não listada — quem tiver o link',
  'Public — listed on AlbionLab': 'Pública — aparece no AlbionLab',
  'Private — only me': 'Privada — só eu',
  "Hide other players' names": 'Esconder o nome dos outros jogadores',
  'Share dungeon location (map names)': 'Compartilhar o local da dungeon (nomes dos mapas)',
  'Disconnect this PC from your AlbionLab account?': 'Desconectar este PC da sua conta do AlbionLab?',
  'Disconnect account': 'Desconectar conta',
  'Name': 'Nome',
  'Untitled session': 'Sessão sem nome',
  'Session name': 'Nome da sessão',
  'Use this name': 'Usar este nome',
  'Suggestion:': 'Sugestão:',
  'Request failed.': 'A requisição falhou.',
  'AlbionLab is unavailable.': 'O AlbionLab está indisponível.',
  'AlbionLab is unreachable: {reason}': 'Não foi possível falar com o AlbionLab: {reason}',
  'AlbionLab did not save the change ({code}).': 'O AlbionLab não salvou a alteração ({code}).',
  'AlbionLab answered {code}.': 'O AlbionLab respondeu {code}.',
  'Unknown visibility.': 'Visibilidade desconhecida.',
  'The name is limited to 80 characters.': 'O nome pode ter no máximo 80 caracteres.',
  'Session not found.': 'Sessão não encontrada.',
  'This session has ended.': 'Esta sessão já terminou.',
  // Record controls (server)
  'Stop the current record before starting another one.': 'Pare a gravação atual antes de começar outra.',
  'There is no active record to stop.': 'Não há gravação ativa para parar.',
  'There is no active record to reset.': 'Não há gravação ativa para zerar.',
  'Press Play to start a record first.': 'Aperte Play para começar uma gravação primeiro.',
  // Capture notice
  'Combat is not being recorded': 'O combate não está sendo gravado',
  ' because {reason}': ' porque {reason}',
  '. Damage and healing from now on will be lost.': '. O dano e a cura a partir de agora serão perdidos.',
  'Open Capture': 'Abrir Captura',
  'the capture storage limit was reached': 'o limite de armazenamento da captura foi atingido',
  'the network interface stopped delivering packets': 'a interface de rede parou de entregar pacotes',
  'an error occurred': 'ocorreu um erro',
  'packet capture permission is missing': 'falta a permissão de captura de pacotes',
  'it was not started': 'ela não foi iniciada',
  '{n} combat updates are waiting for their players to be identified. They are credited as soon as each player is named (chat message, gear change or coming into view) and kept as unidentified players at the next zone change.': '{n} atualizações de combate esperam a identificação dos jogadores. Elas são creditadas assim que cada jogador é nomeado (mensagem no chat, troca de equipamento ou ao aparecer na tela) e ficam como jogadores não identificados na próxima troca de zona.',
  // Capture panel
  'PACKET CAPTURE & REPLAY': 'CAPTURA DE PACOTES E REPLAY',
  'Observe the connection.': 'Observe a conexão.',
  'Observe the network or analyze a saved PCAP / PCAPNG. Starting an interface does not start an encounter; press Play in Overview to record combat.': 'Observe a rede ou analise um PCAP / PCAPNG salvo. Iniciar uma interface não começa um combate; aperte Play no Meter para gravar.',
  'none': 'nenhuma',
  'Capture method': 'Método de captura',
  '{name} (unavailable)': '{name} (indisponível)',
  'Capture interface': 'Interface de captura',
  'Select network interface': 'Selecione a interface de rede',
  'Main': 'Principal',
  'Refresh': 'Atualizar',
  'Start capture': 'Iniciar captura',
  'Replay capture': 'Reproduzir captura',
  'Capture interface access: {error}': 'Acesso às interfaces de captura: {error}',
  'Close Albion Meter and open it again as administrator (right-click AlbionMeter.Server.exe, Run as administrator). Packet capture uses a Windows raw socket and needs no driver.': 'Feche o Albion Meter e abra de novo como administrador (clique com o botão direito em AlbionMeter.Server.exe, Executar como administrador). A captura usa um raw socket do Windows e não precisa de driver.',
  'Albion Meter needs permission to read the game traffic. Allow it once with your system password and capture starts right away.': 'O Albion Meter precisa de permissão para ler o tráfego do jogo. Permita uma vez com a senha do sistema e a captura começa na hora.',
  'Waiting for permission…': 'Esperando a permissão…',
  'Allow packet capture': 'Permitir captura de pacotes',
  'Grant packet capture permission once, then restart Albion Meter:': 'Dê a permissão de captura uma vez e reinicie o Albion Meter:',
  'Copy': 'Copiar',
  'Rebuilding or replacing the executable removes this permission.': 'Recompilar ou substituir o executável remove essa permissão.',
  'Captured frames': 'Frames capturados',
  'Transport messages': 'Mensagens de transporte',
  'Processed traffic': 'Tráfego processado',
  'Adapter drops': 'Perdas do adaptador',
  'Diagnostics': 'Diagnóstico',
  'Decoder counters for the current capture': 'Contadores do decodificador para a captura atual',
  'No capture report yet.': 'Ainda não há relatório de captura.',
  'Session': 'Sessão',
  'Library': 'Biblioteca',
  'Unavailable': 'Indisponível',
  'Combat decoding': 'Decodificação de combate',
  'Live combat': 'Combate ao vivo',
  'Packet persistence': 'Gravação de pacotes',
  'Debug · packets table (bounded)': 'Debug · tabela de pacotes (limitada)',
  'Off · combat data only': 'Desligada · só dados de combate',
  'Live capture does not create PCAP files. The database stores combat timelines, contributions, players, party membership and equipment used by the meter. Replay uses a temporary file which is deleted after analysis.': 'A captura ao vivo não cria arquivos PCAP. O banco guarda as linhas do tempo de combate, contribuições, jogadores, membros da party e equipamentos usados pelo meter. O replay usa um arquivo temporário apagado depois da análise.',
  'Choose a system socket or {library}. Stop capture before changing the method. Socket capture needs administrator rights on Windows or CAP_NET_RAW on Linux.': 'Escolha um socket do sistema ou {library}. Pare a captura antes de trocar o método. A captura por socket precisa de administrador no Windows ou de CAP_NET_RAW no Linux.',
  'Debug mode stores at most 10,000 packets or 32 MB of packet payloads in the packets table. Set Capture__Debug=false and restart to disable it.': 'O modo debug guarda no máximo 10.000 pacotes ou 32 MB de conteúdo na tabela de pacotes. Defina Capture__Debug=false e reinicie para desligar.',
  'Could not list capture interfaces.': 'Não foi possível listar as interfaces de captura.',
  'Capture status is unavailable.': 'O status da captura está indisponível.',
  'Replay files are limited to 40 MB.': 'Arquivos de replay podem ter no máximo 40 MB.',
  'Capture operation failed.': 'A operação de captura falhou.',
  'Packet capture permission was not granted.': 'A permissão de captura de pacotes não foi concedida.',
  'No capture interface is available.': 'Nenhuma interface de captura disponível.',
  'Select a capture interface.': 'Selecione uma interface de captura.',
  'Select an available capture interface.': 'Selecione uma interface de captura disponível.',
  'Select an available socket interface.': 'Selecione uma interface de socket disponível.',
  'The selected interface does not match the capture method.': 'A interface escolhida não combina com o método de captura.',
  'Choose a PCAP or PCAPNG file.': 'Escolha um arquivo PCAP ou PCAPNG.',
  'Choose a capture file smaller than 40 MB.': 'Escolha um arquivo de captura menor que 40 MB.',
  'Packet capture permission denied: run Albion Meter as administrator.': 'Permissão de captura negada: rode o Albion Meter como administrador.',
  'No network adapter with an IPv4 address is up.': 'Nenhum adaptador de rede com endereço IPv4 está ativo.',
  'The file is not a PCAP or PCAPNG capture.': 'O arquivo não é uma captura PCAP ou PCAPNG.',
  'The capture file is truncated.': 'O arquivo de captura está incompleto.',
  // Status pills
  'connecting': 'conectando',
  'idle': 'parada',
  'starting': 'iniciando',
  'running': 'rodando',
  'stopping': 'parando',
  'stopped': 'parada',
  'failed': 'falhou',
  'completed': 'concluída',
  'paused': 'pausada',
  'live': 'ao vivo',
  'live-capture': 'captura ao vivo',
  'replay': 'replay',
  'socket': 'socket',
  'pcap': 'pcap',
  'storageLimit': 'limite de armazenamento',
  'sourceEnded': 'a interface parou',
  'error': 'erro',
  // Game data
  'Game catalog': 'Catálogo do jogo',
  'Game version {version}': 'Versão do jogo {version}',
  'unknown': 'desconhecida',
  'Catalog': 'Catálogo',
  'Weapons': 'Armas',
  'Maps': 'Mapas',
  'Search name, family or ID…': 'Buscar nome, família ou ID…',
  'Search game catalog': 'Buscar no catálogo do jogo',
  'NAME': 'NOME',
  'WEAPON FAMILY': 'FAMÍLIA DA ARMA',
  'MAP TYPE': 'TIPO DE MAPA',
  'TIER': 'TIER',
  'No weapons match your search.': 'Nenhuma arma corresponde à busca.',
  'No maps match your search.': 'Nenhum mapa corresponde à busca.',
  '{n} results': '{n} resultados',
  ' · Showing the first 100, refine your search for more': ' · Mostrando os 100 primeiros, refine a busca para ver mais',
  'Import game data': 'Importar dados do jogo',
  'Select the installed game folder. Only world.bin, items.bin, localization.bin and the version identifier are read. Processing stays on your local server.': 'Selecione a pasta onde o jogo está instalado. Só world.bin, items.bin, localization.bin e o identificador de versão são lidos. O processamento fica no seu computador.',
  'Importing…': 'Importando…',
  'Import game folder': 'Importar pasta do jogo',
  'Weapon definitions': 'Definições de armas',
  'Game version': 'Versão do jogo',
  'Unknown': 'Desconhecida',
  'Imported': 'Importado em',
  'Random dungeon instances may only have a generic name in the game dictionary.': 'Instâncias de dungeons aleatórias podem ter só um nome genérico no dicionário do jogo.',
  'Local records': 'Registros locais',
  'Encounters kept': 'Combates guardados',
  'Dungeon runs kept': 'Dungeons guardadas',
  'No map catalog is available.': 'Nenhum catálogo de mapas disponível.',
  'Storage information unavailable.': 'Informação de armazenamento indisponível.',
  'Reading selected game files…': 'Lendo os arquivos do jogo selecionados…',
  'Select one installed game folder containing GameData/cluster/world.bin.': 'Selecione uma pasta do jogo instalado que contenha GameData/cluster/world.bin.',
  'Import failed.': 'A importação falhou.',
  'Imported {maps} maps and {weapons} weapon definitions.': '{maps} mapas e {weapons} definições de armas importados.',
  'Select a game folder first.': 'Selecione uma pasta do jogo primeiro.',
  'Another import is running.': 'Outra importação já está em andamento.',
  'The game files could not be imported. Select an installed game folder with valid world.bin, items.bin and localization.bin files.': 'Não foi possível importar os arquivos do jogo. Selecione uma pasta do jogo instalado com world.bin, items.bin e localization.bin válidos.',
  // Updates
  'Downloading Albion Meter {version}': 'Baixando o Albion Meter {version}',
  '{done} of {total} · ': '{done} de {total} · ',
  'Keeps running while you play.': 'Continua funcionando enquanto você joga.',
  "What's new": 'Novidades',
  'Albion Meter {version} is ready': 'O Albion Meter {version} está pronto',
  'Restarting stops the current record. If you wait, it installs when you close the meter.': 'Reiniciar para a gravação atual. Se preferir esperar, ele instala quando você fechar o meter.',
  'Later': 'Depois',
  'Restarting…': 'Reiniciando…',
  'Restart and update': 'Reiniciar e atualizar',
  // Copy
  'Copy total damage': 'Copiar dano total',
  'Copy total healing': 'Copiar cura total',
  'Party damage': 'Dano da party',
  'Party healing': 'Cura da party',
  'player': 'jogador',
  'players': 'jogadores',
  'DAMAGE': 'DANO',
  'HEALING': 'CURA',
  'PARTY DPS': 'DPS DA PARTY',
  'PARTY HPS': 'HPS DA PARTY',
  'Total': 'Total',
  'Unarmed': 'Desarmado',
  'Ranking as text': 'Ranking em texto',
  'Ranking as image': 'Ranking em imagem',
  'Player names': 'Nomes dos jogadores',
  'One line per player, the format other meters share in chat': 'Uma linha por jogador, no formato que os outros meters usam no chat',
  'The ranking with weapons and bars, ready to paste': 'O ranking com armas e barras, pronto para colar',
  'Only the names, one per line, in ranking order': 'Só os nomes, um por linha, na ordem do ranking',
  'Ranking copied as text': 'Ranking copiado em texto',
  'Ranking copied as image': 'Ranking copiado em imagem',
  'Player names copied': 'Nomes dos jogadores copiados',
  'Preparing image…': 'Preparando a imagem…',
  'Text clipboard is unavailable in this browser.': 'Não é possível copiar texto aqui.',
  'Image clipboard is unavailable in this browser. Use Copy text.': 'Não é possível copiar imagem aqui. Use o ranking em texto.',
  'Clipboard access was denied. Allow clipboard access and try again.': 'O acesso à área de transferência foi negado. Permita e tente de novo.',
  'Could not copy. Try again.': 'Não foi possível copiar. Tente de novo.',
  'Image rendering is unavailable.': 'Não foi possível desenhar a imagem.',
  'Image rendering failed.': 'A imagem não pôde ser gerada.',
};

/** Messages that carry a value (a status code, a reason) and still deserve a translation. */
const patterns: [RegExp, string][] = [
  [/^AlbionLab is unreachable: (.*)$/s, 'AlbionLab is unreachable: {reason}'],
  [/^AlbionLab did not save the change \((\d+)\)\.$/, 'AlbionLab did not save the change ({code}).'],
  [/^AlbionLab answered (\d+)\.$/, 'AlbionLab answered {code}.'],
];

/** The text in the chosen language, with {name} placeholders filled in. */
export function t(text: string, values?: Record<string, string | number>) {
  let result = language === 'pt' ? pt[text] ?? text : text;
  if (values) for (const [name, value] of Object.entries(values)) result = result.replaceAll(`{${name}}`, String(value));
  return result;
}

/** A message from the capture service or AlbionLab, translated when it is one the meter knows. */
export function message(text: string) {
  if (language !== 'pt' || pt[text]) return t(text);
  for (const [pattern, key] of patterns) {
    const match = pattern.exec(text);
    if (match) return t(key, { reason: match[1], code: match[1] });
  }
  return text;
}
