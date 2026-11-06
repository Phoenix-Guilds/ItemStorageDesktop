import { createClient } from "@supabase/supabase-js";
import CryptoJS from "crypto-js";
import imageCompression from "browser-image-compression";
import {
  CHAT_SESSION_EXPIRY_MS,
  CHAT_SESSION_KEY,
  SUPABASE_ANON_KEY,
  SUPABASE_URL,
} from "./chat-config.js";

const client = createClient(SUPABASE_URL, SUPABASE_ANON_KEY);
const byId = (id) => document.getElementById(id);
const messageInput = byId("chat-message-input");
const messageList = byId("chat-messages");
const historyPanel = byId("chat-history");
const chatScrollButton = byId("chat-scroll-bottom");
const authGate = byId("chat-auth-gate");
const roomForm = byId("chat-room-form");
const personalForm = byId("chat-personal-form");
const selectedFiles = [];
const typingUsers = new Map();
const avatarCache = new Map();
const linkPreviewRequests = new Map();
const replyPreviewRequests = new Map();
let emojiPickerModulePromise = null;

let myNick = "";
let masterKey = "";
let currentUser = null;
let pendingLogin = null;
let oldestTimestamp = null;
let latestMessageTimestamp = null;
let historyLoading = false;
let historyComplete = false;
let replyToId = null;
let editingId = null;
let editingMedia = [];
let typingChannel = null;
let typingSendTimer = null;
let realtimeChannels = [];
let avatarDraft = "";
let reactionPickerRequestId = 0;
let messagePollTimer = null;
let messagePollInFlight = false;
let messagePollErrorReported = false;
let realtimeStatusLogged = false;
let shouldStickToLatest = true;
let messageSizeObserver = null;
let chatScrollButtonVisible = false;

function logChat(message) {
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) {
    console.info(`[CHAT] ${message}`);
    return;
  }
  invoke("log_chat_event", { message }).catch((error) => {
    console.warn("Не удалось записать событие чата в логи:", error);
  });
}

function updateChatScrollButton() {
  const remaining = Math.max(0, latestMessageScrollTop() - messageList.scrollTop);
  if (!chatScrollButtonVisible && remaining >= 600) {
    chatScrollButtonVisible = true;
    chatScrollButton.hidden = false;
  } else if (chatScrollButtonVisible && remaining <= 200) {
    chatScrollButtonVisible = false;
    chatScrollButton.hidden = true;
  }
}

function latestMessageScrollTop() {
  const lastMessage = messageList.querySelector(".chat-message:last-of-type");
  const maxScrollTop = Math.max(0, messageList.scrollHeight - messageList.clientHeight);
  if (!lastMessage || lastMessage.offsetHeight <= messageList.clientHeight) return maxScrollTop;

  const containerTop = messageList.getBoundingClientRect().top;
  const messageTop = lastMessage.getBoundingClientRect().top;
  return Math.max(0, Math.min(maxScrollTop, messageList.scrollTop + messageTop - containerTop));
}

function isNearLatestPosition() {
  return Math.abs(messageList.scrollTop - latestMessageScrollTop()) < 80;
}

function scrollToLatest(behavior = "auto") {
  shouldStickToLatest = true;
  const scroll = () => {
    if (shouldStickToLatest) {
      messageList.scrollTo({ top: latestMessageScrollTop(), behavior });
      updateChatScrollButton();
      clearUnreadIfAtLatest();
    }
  };
  window.requestAnimationFrame(scroll);
  window.requestAnimationFrame(() => window.requestAnimationFrame(scroll));
}

function isChatAtLatest() {
  return Boolean(currentUser)
    && byId("chat-view").classList.contains("is-active")
    && !messageList.hidden
    && !document.hidden
    && document.hasFocus()
    && isNearLatestPosition();
}

function updateUnreadIndicator(unread) {
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) return;
  invoke("set_chat_unread", { unread, atLatest: isChatAtLatest() }).catch((error) => {
    console.warn("Не удалось обновить индикатор новых сообщений:", error);
  });
}

function clearUnreadIfAtLatest() {
  if (isChatAtLatest()) updateUnreadIndicator(false);
}

function observeMessageSize(message) {
  if (!window.ResizeObserver) return;
  if (!messageSizeObserver) {
    messageSizeObserver = new ResizeObserver(() => {
      if (shouldStickToLatest) messageList.scrollTop = latestMessageScrollTop();
      updateChatScrollButton();
    });
  }
  messageSizeObserver.observe(message);
}

function loadEmojiPicker() {
  if (!emojiPickerModulePromise) {
    emojiPickerModulePromise = Promise.all([
      import("emoji-mart"),
      import("@emoji-mart/data"),
    ]).then(([emojiMart, emojiDataModule]) => ({
      Picker: emojiMart.Picker,
      data: emojiDataModule.default,
    })).catch((error) => {
      emojiPickerModulePromise = null;
      throw error;
    });
  }
  return emojiPickerModulePromise;
}

function setConnectionStatus(text, isConnected = false) {
  const status = byId("chat-connection-status");
  status.textContent = text;
  status.classList.toggle("is-connected", isConnected);
}

function setAuthError(message = "") {
  byId("chat-auth-error").textContent = message;
}

function setFormBusy(form, busy) {
  form.querySelectorAll("button, input").forEach((control) => {
    control.disabled = busy;
  });
}

function hashPassword(value) {
  return CryptoJS.SHA256(value).toString();
}

function saveSession() {
  localStorage.setItem(CHAT_SESSION_KEY, JSON.stringify({
    nick: myNick,
    key: masterKey,
    user: {
      id: currentUser.id,
      username: currentUser.username,
      avatar_url: currentUser.avatar_url,
      role: currentUser.role,
    },
    timestamp: Date.now(),
  }));
}

function showPersonalStep() {
  roomForm.hidden = true;
  personalForm.hidden = false;
  byId("chat-auth-title").textContent = "Личный аккаунт";
  byId("chat-auth-description").textContent = "Введите свой логин и личный пароль.";
  byId("chat-nickname").focus();
  setAuthError("");
}

roomForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  setAuthError("");
  setFormBusy(roomForm, true);

  const roomPassword = byId("chat-room-password").value;

  try {
    logChat("Проверка соединения и общего пароля комнаты.");
    const { data: settings, error: settingsError } = await client
      .from("chat_settings")
      .select("password_hash, encrypted_master_key")
      .single();

    if (settingsError) {
      logChat(`Ошибка соединения с Supabase: ${settingsError.message}`);
      throw new Error("Не удалось связаться с чатом. Проверьте подключение.");
    }
    logChat("Соединение с Supabase установлено; параметры комнаты загружены.");
    if (hashPassword(roomPassword) !== settings.password_hash) {
      logChat("Общий пароль комнаты не прошел проверку.");
      throw new Error("Неверный общий пароль чата.");
    }

    const decryptedKey = CryptoJS.AES.decrypt(settings.encrypted_master_key, roomPassword)
      .toString(CryptoJS.enc.Utf8);
    if (!decryptedKey) throw new Error("Не удалось открыть ключ шифрования чата.");

    pendingLogin = { masterKey: decryptedKey };
    byId("chat-room-password").value = "";
    showPersonalStep();
  } catch (error) {
    logChat(`Ошибка первого этапа авторизации: ${error.message}`);
    setAuthError(error.message || "Не удалось выполнить вход.");
  } finally {
    setFormBusy(roomForm, false);
  }
});

personalForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!pendingLogin) return;

  setAuthError("");
  setFormBusy(personalForm, true);
  const nickname = byId("chat-nickname").value.trim();
  const password = byId("chat-personal-password").value;

  try {
    if (!nickname) throw new Error("Введите личный логин.");
    if (!password) throw new Error("Введите личный пароль.");

    logChat("Проверка личного профиля.");
    const { data: existingProfile, error: profileError } = await client
      .from("profiles")
      .select("*")
      .eq("username", nickname)
      .maybeSingle();
    if (profileError) throw new Error("Не удалось проверить личный аккаунт.");
    if (existingProfile?.is_banned) throw new Error("Доступ к чату для этого аккаунта закрыт.");

    if (existingProfile) {
      if (hashPassword(password) !== existingProfile.password_hash) {
        throw new Error("Неверный личный пароль.");
      }
      currentUser = existingProfile;
    } else {
      const { data: profile, error } = await client
        .from("profiles")
        .insert({
          username: nickname,
          password_hash: hashPassword(password),
          avatar_url: `https://api.dicebear.com/7.x/identicon/svg?seed=${encodeURIComponent(nickname)}`,
        })
        .select()
        .single();

      if (error) {
        if (error.code === "23505") throw new Error("Этот ник уже занят. Вернитесь и войдите в аккаунт.");
        throw new Error("Не удалось создать профиль.");
      }
      currentUser = profile;
    }

    myNick = currentUser.username;
    masterKey = pendingLogin.masterKey;
    pendingLogin = null;
    saveSession();
    logChat("Личная авторизация успешна; загружаю историю и подключаю Realtime.");
    await enterChat();
  } catch (error) {
    logChat(`Ошибка личной авторизации: ${error.message}`);
    setAuthError(error.message || "Не удалось выполнить вход.");
  } finally {
    setFormBusy(personalForm, false);
  }
});

byId("chat-back-to-room").addEventListener("click", () => {
  pendingLogin = null;
  personalForm.hidden = true;
  roomForm.hidden = false;
  byId("chat-auth-title").textContent = "Вход в чат гильдии";
  byId("chat-auth-description").textContent = "Сначала подтвердите доступ общим паролем чата.";
  byId("chat-nickname").value = "";
  byId("chat-personal-password").value = "";
  setAuthError("");
  byId("chat-room-password").focus();
});

async function restoreSession() {
  try {
    const raw = localStorage.getItem(CHAT_SESSION_KEY);
    if (!raw) return;
    logChat("Обнаружена сохраненная сессия; проверяю профиль.");
    const session = JSON.parse(raw);
    if (Date.now() - session.timestamp > CHAT_SESSION_EXPIRY_MS) {
      localStorage.removeItem(CHAT_SESSION_KEY);
      return;
    }

    myNick = session.nick;
    masterKey = session.key;
    currentUser = session.user;

    const { data: profile, error } = await client
      .from("profiles")
      .select("id, username, avatar_url, role, is_banned")
      .eq("id", currentUser.id)
      .maybeSingle();
    if (error) throw error;
    if (!profile || profile.is_banned) {
      localStorage.removeItem(CHAT_SESSION_KEY);
      myNick = "";
      masterKey = "";
      currentUser = null;
      setAuthError("Сессия завершена. Войдите в чат снова.");
      return;
    }
    currentUser = profile;
    myNick = profile.username;
    saveSession();
    logChat("Сохраненная сессия подтверждена.");
    await enterChat();
  } catch (error) {
    logChat(`Не удалось восстановить сессию: ${error.message || error}`);
    console.warn("Не удалось восстановить сессию чата:", error);
    localStorage.removeItem(CHAT_SESSION_KEY);
    setAuthError("Не удалось восстановить сессию. Войдите в чат снова.");
  }
}

async function enterChat() {
  authGate.hidden = true;
  messageList.hidden = false;
  historyPanel.classList.add("is-ready");
  byId("chat-user-tools").hidden = false;
  messageInput.disabled = false;
  byId("chat-send-button").disabled = false;
  byId("chat-attach-button").disabled = false;
  byId("chat-emoji-button").disabled = false;
  byId("chat-user-name").textContent = currentUser.username;
  byId("chat-user-avatar").src = currentUser.avatar_url || makeAvatarUrl(currentUser.username);
  updateMuteLabel();
  setConnectionStatus("Загрузка сообщений...");
  await loadHistory();
  scrollToLatest();
  connectRealtime();
  if (!messagePollTimer) {
     logChat("Резервная проверка новых сообщений запущена (интервал 30 секунд).");
     messagePollTimer = window.setInterval(pollLatestMessages, 30000);
  }
}

function messageDateKey(timestamp) {
  const date = new Date(timestamp);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function formatMessageDate(timestamp) {
  const date = new Date(timestamp);
  const today = new Date();
  if (messageDateKey(date) === messageDateKey(today)) return "Сегодня";

  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  if (messageDateKey(date) === messageDateKey(yesterday)) return "Вчера";

  const dayAndMonth = new Intl.DateTimeFormat("ru-RU", {
    day: "numeric",
    month: "long",
  }).format(date);
  return date.getFullYear() === today.getFullYear()
    ? dayAndMonth
    : `${dayAndMonth} ${date.getFullYear()}`;
}

function createDateSeparator(timestamp) {
  const separator = document.createElement("div");
  separator.className = "chat-date-separator";
  separator.dataset.dateKey = messageDateKey(timestamp);
  separator.textContent = formatMessageDate(timestamp);
  return separator;
}

function appendMessageWithDate(container, message, node, previousDateKey) {
  if (node.dataset.dateKey !== previousDateKey) {
    container.append(createDateSeparator(message.created_at));
  }
  container.append(node);
  return node.dataset.dateKey;
}

async function loadHistory(older = false) {
  if (!currentUser || historyLoading || historyComplete) return;
  historyLoading = true;
  const oldHeight = messageList.scrollHeight;

  try {
    if (!older) logChat("Загружаю последние сообщения.");
    let query = client
      .from("messages")
      .select("*, reactions(*), author_name:author, author:profiles(avatar_url)")
      .order("created_at", { ascending: false })
      .limit(40);
    if (oldestTimestamp) query = query.lt("created_at", oldestTimestamp);

    const { data, error } = await query;
    if (error) throw error;
    if (!data || data.length === 0) {
      historyComplete = true;
      if (!older) {
        logChat("История чата загружена: сообщений пока нет.");
        setConnectionStatus("Чат подключен", true);
      }
      return;
    }

    oldestTimestamp = data[data.length - 1].created_at;
    historyComplete = data.length < 40;
    const rows = data.slice().reverse();
    if (!older && rows.length) latestMessageTimestamp = rows[rows.length - 1].created_at;

    if (older) {
      const firstMessage = messageList.querySelector(".chat-message");
      const existingSeparator = firstMessage?.previousElementSibling;
      if (existingSeparator?.classList.contains("chat-date-separator")) {
        existingSeparator.remove();
      }
      const fragment = document.createDocumentFragment();
      let previousDateKey = null;
      const messageNodes = rows.map((message) => {
        const node = renderMessage(message);
        previousDateKey = appendMessageWithDate(fragment, message, node, previousDateKey);
        return node;
      });
      messageList.insertBefore(fragment, messageList.firstChild);
      messageNodes.forEach(observeMessageSize);
      messageList.scrollTop += messageList.scrollHeight - oldHeight;
    } else {
      let previousDateKey = null;
      rows.forEach((message) => {
        const node = renderMessage(message);
        previousDateKey = appendMessageWithDate(messageList, message, node, previousDateKey);
        observeMessageSize(node);
      });
      scrollToLatest();
    }
    logChat(older
      ? `Загружены предыдущие сообщения: ${data.length}.`
      : `Последние сообщения загружены: ${data.length}.`);
    setConnectionStatus("Чат подключен", true);
  } catch (error) {
    logChat(`Ошибка загрузки истории: ${error.message || error}`);
    setConnectionStatus("Нет связи с чатом");
    if (!older) showChatNotice(error.message || "Не удалось загрузить сообщения.");
  } finally {
    historyLoading = false;
  }
}

function connectRealtime() {
  if (realtimeChannels.length) return;

  const messagesChannel = client
    .channel("messages_changes")
    .on("postgres_changes", { event: "*", schema: "public", table: "messages" }, async (change) => {
      if (change.eventType === "DELETE") {
        const messageNode = byId(`chat-message-${change.old.id}`);
        const separator = messageNode?.previousElementSibling;
        messageNode?.remove();
        if (
          separator?.classList.contains("chat-date-separator")
          && separator.nextElementSibling?.dataset.dateKey !== separator.dataset.dateKey
        ) {
          separator.remove();
        }
        logChat("Сообщение удалено через Realtime.");
        return;
      }
      const { data, error } = await client
        .from("messages")
        .select("*, reactions(*), author_name:author, author:profiles(avatar_url)")
        .eq("id", change.new.id)
        .single();
      if (error || !data) {
        if (error) logChat(`Не удалось загрузить Realtime-сообщение: ${error.message}`);
        return;
      }
      const wasAtLatest = change.eventType === "INSERT" && isChatAtLatest();
      const inserted = upsertMessage(data);
      if (change.eventType === "INSERT") {
        logChat("Новое сообщение получено через Realtime.");
        if (inserted && data.author_name !== myNick) {
          playNotification();
          updateUnreadIndicator(!wasAtLatest);
        }
      } else {
        logChat("Сообщение обновлено через Realtime.");
      }
    })
    .subscribe((status) => {
      if (status === "SUBSCRIBED") {
        setConnectionStatus("Чат подключен", true);
        if (!realtimeStatusLogged) logChat("Realtime-подписка на сообщения установлена.");
        realtimeStatusLogged = true;
      }
      if (status === "CHANNEL_ERROR" || status === "TIMED_OUT" || status === "CLOSED") {
        setConnectionStatus("Восстанавливаю связь...");
        if (realtimeStatusLogged) logChat(`Realtime сообщений: ${status}; включена резервная проверка.`);
        realtimeStatusLogged = false;
      }
    });

  const reactionsChannel = client
    .channel("reactions_changes")
    .on("postgres_changes", { event: "*", schema: "public", table: "reactions" }, async (change) => {
      const messageId = change.new?.message_id || change.old?.message_id;
      if (messageId) await refreshReactions(messageId);
    })
    .subscribe((status) => {
      if (status === "SUBSCRIBED") logChat("Realtime-подписка на реакции установлена.");
      if (status === "CHANNEL_ERROR" || status === "TIMED_OUT") logChat(`Ошибка Realtime-канала реакций: ${status}.`);
    });

  typingChannel = client.channel("chat-room-1");
  typingChannel
    .on("broadcast", { event: "typing" }, ({ payload }) => {
      const username = payload?.user;
      if (!username || username === myNick) return;
      typingUsers.set(username, Date.now());
      updateTypingIndicator();
      window.setTimeout(() => {
        if (Date.now() - (typingUsers.get(username) || 0) >= 3000) {
          typingUsers.delete(username);
          updateTypingIndicator();
        }
      }, 3100);
    })
    .subscribe((status) => {
      if (status === "SUBSCRIBED") logChat("Realtime-канал индикатора набора подключен.");
      if (status === "CHANNEL_ERROR" || status === "TIMED_OUT") logChat(`Ошибка Realtime-канала набора: ${status}.`);
    });

  realtimeChannels = [messagesChannel, reactionsChannel, typingChannel];
}

async function pollLatestMessages() {
  if (!currentUser || messagePollInFlight) return;
  messagePollInFlight = true;

  try {
    let query = client
      .from("messages")
      .select("*, reactions(*), author_name:author, author:profiles(avatar_url)")
      .order("created_at", { ascending: latestMessageTimestamp !== null })
      .limit(100);
    if (latestMessageTimestamp) query = query.gte("created_at", latestMessageTimestamp);

    const { data, error } = await query;
    if (error) throw error;

    const rows = latestMessageTimestamp ? (data || []) : (data || []).slice().reverse();
    const unseen = rows
      .filter((message) => !byId(`chat-message-${message.id}`))
      .sort((left, right) => new Date(left.created_at) - new Date(right.created_at));
    const hasIncoming = unseen.some((message) => message.author_name !== myNick);
    const wasAtLatest = hasIncoming && isChatAtLatest();
    unseen.forEach((message) => upsertMessage(message));
    if (rows.length) latestMessageTimestamp = rows[rows.length - 1].created_at;

    if (unseen.length) {
      logChat(`Резервная проверка получила новые сообщения: ${unseen.length}.`);
      if (hasIncoming) {
        playNotification();
        updateUnreadIndicator(!wasAtLatest);
      }
    }
    if (messagePollErrorReported) {
      logChat("Резервное соединение с Supabase восстановлено.");
      messagePollErrorReported = false;
      setConnectionStatus("Чат подключен", true);
    }
  } catch (error) {
    setConnectionStatus("Восстанавливаю связь...");
    if (!messagePollErrorReported) {
      logChat(`Ошибка резервной проверки сообщений: ${error.message || error}`);
      messagePollErrorReported = true;
    }
  } finally {
    messagePollInFlight = false;
  }
}

function upsertMessage(message) {
  const existing = byId(`chat-message-${message.id}`);
  const wasNearLatest = isNearLatestPosition();
  const node = renderMessage(message);
  if (existing) {
    messageSizeObserver?.unobserve(existing);
    existing.replaceWith(node);
  } else {
    const lastMessage = messageList.querySelector(".chat-message:last-of-type");
    const previousDateKey = lastMessage?.dataset.dateKey || null;
    appendMessageWithDate(messageList, message, node, previousDateKey);
  }
  observeMessageSize(node);
  if (!latestMessageTimestamp || new Date(message.created_at) > new Date(latestMessageTimestamp)) {
    latestMessageTimestamp = message.created_at;
  }
  if (!existing && wasNearLatest) scrollToLatest();
  else updateChatScrollButton();
  return !existing;
}

function decryptMessage(payload) {
  const bytes = CryptoJS.AES.decrypt(payload, masterKey);
  const decoded = bytes.toString(CryptoJS.enc.Utf8);
  if (!decoded) throw new Error("Пустой результат дешифрования");
  try {
    const parsed = JSON.parse(decoded);
    if (parsed && typeof parsed === "object") {
      return { text: String(parsed.text || ""), media: Array.isArray(parsed.media) ? parsed.media : [] };
    }
  } catch {
    // Старые сообщения сайта шифровали просто текст.
  }
  return { text: decoded, media: [] };
}

function safeMediaUrl(value) {
  const url = typeof value === "string" ? value : value?.url;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" ? parsed.href : null;
  } catch {
    return null;
  }
}

function appendLinkedText(container, text) {
  const pattern = /https?:\/\/[^\s]+/g;
  let cursor = 0;
  let firstUrl = null;
  for (const match of text.matchAll(pattern)) {
    const start = match.index;
    const rawUrl = match[0];
    const trailing = rawUrl.match(/[),.!?;:]+$/)?.[0] || "";
    const url = rawUrl.slice(0, rawUrl.length - trailing.length);
    if (start > cursor) container.append(document.createTextNode(text.slice(cursor, start)));
    try {
      const parsed = new URL(url);
      const link = document.createElement("a");
      link.href = parsed.href;
      link.target = "_blank";
      link.rel = "noreferrer";
      link.textContent = url;
      container.append(link);
      if (!firstUrl) firstUrl = parsed.href;
      if (trailing) container.append(document.createTextNode(trailing));
    } catch {
      container.append(document.createTextNode(rawUrl));
    }
    cursor = start + rawUrl.length;
  }
  if (cursor < text.length) container.append(document.createTextNode(text.slice(cursor)));
  return firstUrl;
}

function renderMessage(message) {
  const node = document.createElement("article");
  node.className = "chat-message";
  node.id = `chat-message-${message.id}`;
  node.dataset.createdAt = message.created_at;
  node.dataset.dateKey = messageDateKey(message.created_at);

  let authorName = message.author_name || message.author;
  let authorAvatar = message.author?.avatar_url || avatarCache.get(authorName);
  if (authorName && typeof authorName === "object") authorName = authorName.username || "Участник";
  authorName = String(authorName || "Участник");
  if (authorName.toLowerCase() === myNick.toLowerCase()) authorAvatar = currentUser?.avatar_url || authorAvatar;
  if (!authorAvatar) authorAvatar = makeAvatarUrl(authorName);
  avatarCache.set(authorName, authorAvatar);
  const isOwn = authorName.toLowerCase() === myNick.toLowerCase();
  if (isOwn) node.classList.add("is-own");

  let content;
  try {
    content = decryptMessage(message.payload);
  } catch {
    content = { text: "🔒 Не удалось расшифровать сообщение", media: [] };
  }

  const meta = document.createElement("div");
  meta.className = "chat-message-meta";
  const avatar = document.createElement("img");
  avatar.className = "chat-message-avatar";
  avatar.src = authorAvatar;
  avatar.alt = "";
  const author = document.createElement("span");
  author.className = "chat-message-author";
  author.textContent = authorName;
  const time = document.createElement("time");
  time.className = "chat-message-time";
  time.dateTime = message.created_at;
  time.textContent = new Date(message.created_at).toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
  meta.append(avatar, author, time);
  if (message.is_edited) {
    const edited = document.createElement("span");
    edited.className = "chat-edited-mark";
    edited.textContent = "изм.";
    meta.append(edited);
  }
  node.append(meta);

  if (message.reply_to_id) {
    const quote = document.createElement("button");
    quote.type = "button";
    quote.className = "chat-reply-quote";
    const quoteAuthor = document.createElement("span");
    quoteAuthor.className = "chat-reply-author";
    quoteAuthor.textContent = "↩ Ответ на сообщение";
    const quoteText = document.createElement("span");
    quoteText.className = "chat-reply-text";
    quoteText.textContent = "Загрузка исходного сообщения...";
    quote.append(quoteAuthor, quoteText);
    quote.addEventListener("click", () => {
      const target = byId(`chat-message-${message.reply_to_id}`);
      target?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
    node.append(quote);
    loadReplyPreview(message.reply_to_id).then((reply) => {
      if (!quote.isConnected) return;
      if (!reply) {
        quoteText.textContent = "Исходное сообщение недоступно";
        return;
      }
      quoteAuthor.textContent = `↩ ${reply.author}`;
      quoteText.textContent = reply.text.replace(/\s+/g, " ").trim().slice(0, 120) || "Вложение";
    });
  }

  if (content.media.length) {
    const media = document.createElement("div");
    media.className = "chat-message-media";
    for (const item of content.media) {
      const url = safeMediaUrl(item);
      if (!url) continue;
      const link = document.createElement("a");
      link.href = url;
      link.target = "_blank";
      link.rel = "noreferrer";
      const image = document.createElement("img");
      image.src = url;
      image.loading = "lazy";
      image.alt = "Изображение в сообщении";
      image.addEventListener("load", () => {
        if (shouldStickToLatest) scrollToLatest();
      }, { once: true });
      link.append(image);
      media.append(link);
    }
    if (media.childElementCount) node.append(media);
  }

  const body = document.createElement("div");
  body.className = "chat-message-body";
  const firstUrl = appendLinkedText(body, content.text);
  node.append(body);
  if (firstUrl) {
    const preview = document.createElement("div");
    preview.className = "chat-link-preview-slot";
    node.append(preview);
    void loadLinkPreview(firstUrl, preview);
  }

  const actions = document.createElement("div");
  actions.className = "chat-message-actions";
  actions.append(messageActionButton("☺", "Реакция", (event) => showReactionPicker(event, message.id)));
  actions.append(messageActionButton("↩", "Ответить", () => prepareReply(message, authorName, content.text)));
  if (isOwn) {
    actions.append(messageActionButton("✎", "Изменить", () => prepareEdit(message.id, content.text, content.media)));
    actions.append(messageActionButton("×", "Удалить", () => deleteMessage(message)));
  }
  node.append(actions);

  const reactions = document.createElement("div");
  reactions.className = "chat-reactions";
  renderReactions(reactions, message.id, message.reactions || []);
  node.append(reactions);
  return node;
}

function loadReplyPreview(messageId) {
  if (!replyPreviewRequests.has(messageId)) {
    const request = client
      .from("messages")
      .select("author, payload")
      .eq("id", messageId)
      .maybeSingle()
      .then(({ data, error }) => {
        if (error || !data) return null;
        try {
          const content = decryptMessage(data.payload);
          return { author: String(data.author || "Участник"), text: content.text };
        } catch {
          return null;
        }
      })
      .catch(() => null);
    replyPreviewRequests.set(messageId, request);
  }
  return replyPreviewRequests.get(messageId);
}

async function loadLinkPreview(url, container) {
  try {
    let request = linkPreviewRequests.get(url);
    if (!request) {
      request = (async () => {
        const controller = new AbortController();
        const timeout = window.setTimeout(() => controller.abort(), 7000);
        try {
          const response = await fetch(
            `https://opengraph.ninja/api/v1?url=${encodeURIComponent(url)}`,
            { signal: controller.signal },
          );
          if (!response.ok) return null;
          return await response.json();
        } finally {
          window.clearTimeout(timeout);
        }
      })();
      linkPreviewRequests.set(url, request);
    }

    const data = await request;
    if (!data || (!data.title && !data.image)) return;

    const previewLink = document.createElement("a");
    previewLink.className = "chat-link-preview";
    previewLink.href = url;
    previewLink.target = "_blank";
    previewLink.rel = "noreferrer";

    const imageUrl = safeMediaUrl(data.image?.url || data.image);
    if (imageUrl) {
      const image = document.createElement("img");
      image.src = imageUrl;
      image.alt = "";
      image.loading = "lazy";
      previewLink.append(image);
    }

    const copy = document.createElement("div");
    copy.className = "chat-link-preview-copy";
    if (data.title) {
      const title = document.createElement("div");
      title.className = "chat-link-preview-title";
      title.textContent = data.title;
      copy.append(title);
    }
    if (data.description) {
      const description = document.createElement("div");
      description.className = "chat-link-preview-description";
      description.textContent = data.description;
      copy.append(description);
    }
    const site = document.createElement("div");
    site.className = "chat-link-preview-site";
    site.textContent = new URL(url).hostname;
    copy.append(site);
    previewLink.append(copy);
    container.replaceChildren(previewLink);
  } catch {
    linkPreviewRequests.delete(url);
  }
}

function actionButton(label, title, action) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  button.title = title;
  button.setAttribute("aria-label", title);
  button.addEventListener("click", action);
  return button;
}

function messageActionButton(label, title, action) {
  const button = actionButton(label, title, action);
  button.classList.add("chat-message-action-button");
  return button;
}

function renderReactions(container, messageId, reactions) {
  container.replaceChildren();
  const grouped = new Map();
  for (const reaction of reactions) {
    if (!grouped.has(reaction.emoji)) grouped.set(reaction.emoji, []);
    grouped.get(reaction.emoji).push(reaction);
  }
  for (const [emoji, users] of grouped) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `chat-reaction${users.some((item) => item.user_id === currentUser?.id) ? " is-mine" : ""}`;
    button.title = users.map((item) => item.user_name).join(", ");
    const emojiLabel = document.createElement("span");
    emojiLabel.className = "chat-reaction-emoji";
    emojiLabel.textContent = emoji;
    const count = document.createElement("span");
    count.className = "chat-reaction-count";
    count.textContent = String(users.length);
    const avatars = document.createElement("span");
    avatars.className = "chat-reaction-avatars";
    users.slice(0, 3).forEach((user) => {
      const avatarUrl = safeMediaUrl(user.user_avatar);
      if (!avatarUrl) return;
      const avatar = document.createElement("img");
      avatar.className = "chat-reaction-avatar";
      avatar.src = avatarUrl;
      avatar.alt = "";
      avatar.title = user.user_name || "Участник";
      avatars.append(avatar);
    });
    button.append(emojiLabel, count, avatars);
    button.addEventListener("click", () => toggleReaction(messageId, emoji));
    container.append(button);
  }
}

async function refreshReactions(messageId) {
  const { data, error } = await client.from("reactions").select("*").eq("message_id", messageId);
  if (error) return;
  const node = byId(`chat-message-${messageId}`);
  const container = node?.querySelector(".chat-reactions");
  if (container) renderReactions(container, messageId, data || []);
}

async function toggleReaction(messageId, emoji) {
  const { data: existing, error: lookupError } = await client
    .from("reactions")
    .select("id, emoji")
    .eq("message_id", messageId)
    .eq("user_id", currentUser.id)
    .maybeSingle();
  if (lookupError) return showChatNotice("Не удалось загрузить реакцию.");

  let result;
  if (existing?.emoji === emoji) {
    result = await client.from("reactions").delete().eq("id", existing.id);
  } else if (existing) {
    result = await client.from("reactions").update({ emoji }).eq("id", existing.id);
  } else {
    result = await client.from("reactions").insert({
      message_id: messageId,
      user_id: currentUser.id,
      user_name: currentUser.username,
      user_avatar: currentUser.avatar_url,
      emoji,
    });
  }
  if (result.error) showChatNotice("Не удалось сохранить реакцию.");
  else await refreshReactions(messageId);
}

let reactionPickerOutsideHandler = null;

function closeReactionPicker() {
  reactionPickerRequestId += 1;
  const container = byId("chat-reaction-picker-container");
  container.hidden = true;
  container.replaceChildren();
  if (reactionPickerOutsideHandler) {
    document.removeEventListener("pointerdown", reactionPickerOutsideHandler, true);
    reactionPickerOutsideHandler = null;
  }
}

async function showReactionPicker(event, messageId) {
  closeReactionPicker();
  const requestId = reactionPickerRequestId;
  const container = byId("chat-reaction-picker-container");
  container.hidden = false;
  container.textContent = "Загрузка эмодзи...";

  const pickerWidth = Math.min(352, window.innerWidth - 24);
  const pickerHeight = Math.min(435, window.innerHeight - 24);
  const left = Math.max(12, Math.min(event.clientX, window.innerWidth - pickerWidth - 12));
  let top = event.clientY + 10;
  if (top + pickerHeight > window.innerHeight - 12) top = event.clientY - pickerHeight - 10;
  container.style.left = `${left}px`;
  container.style.top = `${Math.max(12, top)}px`;

  try {
    const { Picker, data } = await loadEmojiPicker();
    if (requestId !== reactionPickerRequestId || container.hidden) return;
    container.replaceChildren(new Picker({
      data,
      theme: "dark",
      locale: "ru",
      set: "native",
      onEmojiSelect: async (emoji) => {
        closeReactionPicker();
        await toggleReaction(messageId, emoji.native);
      },
    }));
  } catch {
    if (requestId === reactionPickerRequestId) {
      container.textContent = "Не удалось загрузить эмодзи.";
    }
    return;
  }

  reactionPickerOutsideHandler = (pointerEvent) => {
    if (!container.contains(pointerEvent.target) && !pointerEvent.target.closest(".chat-message-actions")) {
      closeReactionPicker();
    }
  };
  window.setTimeout(() => {
    if (!container.hidden) document.addEventListener("pointerdown", reactionPickerOutsideHandler, true);
  }, 0);
}

function prepareReply(message, author, text) {
  editingId = null;
  replyToId = message.id;
  byId("chat-reply-label").textContent = `Ответ ${author}: ${text.slice(0, 100)}`;
  byId("chat-reply-preview").hidden = false;
  messageInput.focus();
}

function prepareEdit(messageId, text, media = []) {
  replyToId = null;
  editingId = messageId;
  editingMedia = media.slice();
  byId("chat-reply-label").textContent = "Редактирование сообщения";
  byId("chat-reply-preview").hidden = false;
  messageInput.value = text;
  messageInput.focus();
}

function clearComposeMode() {
  replyToId = null;
  editingId = null;
  editingMedia = [];
  byId("chat-reply-preview").hidden = true;
  byId("chat-reply-label").textContent = "";
}

async function deleteMessage(message) {
  const confirmed = await confirmChatAction("Удалить это сообщение?", {
    title: "Удаление сообщения",
    confirmText: "Удалить",
  });
  if (!confirmed) return;
  try {
    const content = decryptMessage(message.payload);
    const paths = content.media.map((item) => {
      const url = safeMediaUrl(item);
      if (!url) return null;
      const marker = "/chat-media/";
      const index = url.indexOf(marker);
      return index < 0 ? null : decodeURIComponent(url.slice(index + marker.length));
    }).filter(Boolean);

    if (paths.length) {
      const { error: storageError } = await client.storage.from("chat-media").remove(paths);
      if (storageError) console.warn("Не удалось удалить вложения:", storageError);
    }
    const { error } = await client.from("messages").delete().eq("id", message.id);
    if (error) throw error;
    byId(`chat-message-${message.id}`)?.remove();
  } catch (error) {
    showChatNotice(error.message || "Не удалось удалить сообщение.");
  }
}

function confirmChatAction(message, { title = "Подтверждение", confirmText = "Подтвердить" } = {}) {
  return new Promise((resolve) => {
    const modal = byId("chat-confirm-modal");
    const acceptButton = byId("chat-confirm-accept");
    const cancelButton = byId("chat-confirm-cancel");
    let settled = false;

    byId("chat-confirm-title").textContent = title;
    byId("chat-confirm-message").textContent = message;
    acceptButton.textContent = confirmText;

    const finish = (accepted) => {
      if (settled) return;
      settled = true;
      modal.hidden = true;
      acceptButton.removeEventListener("click", onAccept);
      cancelButton.removeEventListener("click", onCancel);
      modal.removeEventListener("click", onBackdrop);
      window.removeEventListener("keydown", onKeyDown);
      resolve(accepted);
    };
    const onAccept = () => finish(true);
    const onCancel = () => finish(false);
    const onBackdrop = (event) => {
      if (event.target === modal) finish(false);
    };
    const onKeyDown = (event) => {
      if (event.key === "Escape") finish(false);
    };

    acceptButton.addEventListener("click", onAccept);
    cancelButton.addEventListener("click", onCancel);
    modal.addEventListener("click", onBackdrop);
    window.addEventListener("keydown", onKeyDown);
    modal.hidden = false;
    cancelButton.focus();
  });
}

async function sendMessage() {
  const text = messageInput.value.trim();
  if ((!text && selectedFiles.length === 0) || !currentUser) return;

  const isEditing = Boolean(editingId);
  const sendButton = byId("chat-send-button");
  sendButton.disabled = true;
  logChat(isEditing ? "Сохранение изменений сообщения." : "Отправка сообщения.");
  try {
    const media = editingId ? editingMedia.slice() : [];
    if (!editingId) {
      for (const file of selectedFiles) {
        const extension = file.name.split(".").pop().replace(/[^a-z0-9]/gi, "").slice(0, 8) || "img";
        const path = `public/${Date.now()}_${crypto.randomUUID()}.${extension}`;
        const { error: uploadError } = await client.storage.from("chat-media").upload(path, file);
        if (uploadError) throw uploadError;
        const { data } = client.storage.from("chat-media").getPublicUrl(path);
        media.push({ url: data.publicUrl, ...(await imageSize(file)) });
      }
    }

    const encrypted = CryptoJS.AES.encrypt(
      JSON.stringify({ text, media }),
      masterKey,
    ).toString();

    if (editingId) {
      const { error } = await client.from("messages")
        .update({ payload: encrypted, is_edited: true })
        .eq("id", editingId);
      if (error) throw error;
    } else {
      const { error } = await client.from("messages").insert({
        author: myNick,
        payload: encrypted,
        reply_to_id: replyToId,
      });
      if (error) throw error;
    }

    messageInput.value = "";
    selectedFiles.splice(0);
    byId("chat-file-input").value = "";
    byId("chat-attachment-previews").replaceChildren();
    clearComposeMode();
    resizeComposer();
    logChat(isEditing ? "Сообщение изменено успешно." : "Сообщение отправлено успешно.");
  } catch (error) {
    logChat(`Ошибка отправки сообщения: ${error.message || error}`);
    showChatNotice(error.message || "Не удалось отправить сообщение.");
  } finally {
    sendButton.disabled = false;
    messageInput.focus();
  }
}

function imageSize(file) {
  return new Promise((resolve) => {
    const image = new Image();
    const url = URL.createObjectURL(file);
    image.onload = () => {
      resolve({ w: image.naturalWidth || 1200, h: image.naturalHeight || 800 });
      URL.revokeObjectURL(url);
    };
    image.onerror = () => {
      resolve({ w: 1200, h: 800 });
      URL.revokeObjectURL(url);
    };
    image.src = url;
  });
}

async function handleFileSelection(event) {
  const files = Array.from(event.target.files || []);
  if (selectedFiles.length + files.length > 10) {
    showChatNotice("Можно прикрепить не более 10 изображений.");
    event.target.value = "";
    return;
  }
  for (const file of files) {
    if (!file.type.startsWith("image/")) continue;
    try {
      const prepared = file.type === "image/gif"
        ? file
        : await imageCompression(file, { maxSizeMB: 2, maxWidthOrHeight: 1920, useWebWorker: true });
      selectedFiles.push(prepared);
    } catch (error) {
      console.warn("Не удалось подготовить изображение:", error);
    }
  }
  event.target.value = "";
  renderAttachmentPreviews();
}

function renderAttachmentPreviews() {
  const container = byId("chat-attachment-previews");
  container.replaceChildren();
  selectedFiles.forEach((file, index) => {
    const wrapper = document.createElement("div");
    wrapper.className = "chat-attachment-preview";
    const image = document.createElement("img");
    image.src = URL.createObjectURL(file);
    image.alt = file.name;
    const remove = actionButton("×", "Убрать изображение", () => {
      selectedFiles.splice(index, 1);
      renderAttachmentPreviews();
    });
    wrapper.append(image, remove);
    container.append(wrapper);
  });
}

function setupComposer() {
  resizeComposer();
  byId("chat-send-button").addEventListener("click", sendMessage);
  messageInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      sendMessage();
    }
  });
  messageInput.addEventListener("input", () => {
    resizeComposer();
    if (!typingChannel || typingSendTimer) return;
    typingChannel.send({ type: "broadcast", event: "typing", payload: { user: myNick } });
    typingSendTimer = window.setTimeout(() => { typingSendTimer = null; }, 2000);
  });
  byId("chat-attach-button").addEventListener("click", () => byId("chat-file-input").click());
  byId("chat-file-input").addEventListener("change", handleFileSelection);
  byId("chat-cancel-reply").addEventListener("click", () => {
    clearComposeMode();
    messageInput.value = "";
    resizeComposer();
  });
  byId("chat-messages").addEventListener("scroll", () => {
    if (messageList.scrollHeight > messageList.clientHeight) {
      shouldStickToLatest = isNearLatestPosition();
    }
    updateChatScrollButton();
    clearUnreadIfAtLatest();
    if (messageList.scrollTop < 50) loadHistory(true);
  });
  chatScrollButton.addEventListener("click", () => scrollToLatest("smooth"));
  document.querySelectorAll("[data-view]").forEach((button) => {
    button.addEventListener("click", () => {
      if (button.dataset.view === "chat-view") {
        window.requestAnimationFrame(clearUnreadIfAtLatest);
      }
    });
  });
  window.addEventListener("focus", clearUnreadIfAtLatest);
  document.addEventListener("visibilitychange", clearUnreadIfAtLatest);

  const emojiMenu = byId("chat-emoji-menu");
  let composerPickerPromise = null;
  byId("chat-emoji-button").addEventListener("click", async () => {
    if (!emojiMenu.hidden) {
      emojiMenu.hidden = true;
      return;
    }

    emojiMenu.hidden = false;
    if (emojiMenu.childElementCount) return;

    if (!composerPickerPromise) {
      composerPickerPromise = (async () => {
        const { Picker, data } = await loadEmojiPicker();
        if (emojiMenu.hidden || emojiMenu.childElementCount) return;
        emojiMenu.replaceChildren(new Picker({
          data,
          theme: "dark",
          locale: "ru",
          set: "native",
          onEmojiSelect: (emoji) => {
            const start = messageInput.selectionStart;
            const end = messageInput.selectionEnd;
            messageInput.setRangeText(emoji.native, start, end, "end");
            messageInput.focus();
            resizeComposer();
            emojiMenu.hidden = true;
          },
        }));
      })().catch(() => {
        if (!emojiMenu.hidden) emojiMenu.textContent = "Не удалось загрузить эмодзи.";
      }).finally(() => {
        composerPickerPromise = null;
      });
    }
    await composerPickerPromise;
  });
  document.addEventListener("pointerdown", (event) => {
    if (!emojiMenu.contains(event.target) && event.target !== byId("chat-emoji-button")) {
      emojiMenu.hidden = true;
    }
  });
}

function resizeComposer() {
  messageInput.style.height = "34px";
  const measuredHeight = messageInput.scrollHeight + 2;
  const nextHeight = Math.max(34, Math.min(measuredHeight, 120));
  messageInput.style.height = `${nextHeight}px`;
  messageInput.style.overflowY = measuredHeight > 120 ? "auto" : "hidden";
}

function updateTypingIndicator() {
  const now = Date.now();
  for (const [name, timestamp] of typingUsers) {
    if (now - timestamp > 3500) typingUsers.delete(name);
  }
  const names = [...typingUsers.keys()];
  const text = names.length === 1
    ? `${names[0]} печатает...`
    : names.length === 2
      ? `${names[0]} и ${names[1]} печатают...`
      : names.length > 2
        ? `${names[0]}, ${names[1]} и еще ${names.length - 2} печатают...`
        : "";
  byId("chat-typing").textContent = text;
}

function updateMuteLabel() {
  const until = Number(localStorage.getItem("chat_mute_until") || 0);
  const summary = document.querySelector(".chat-preferences summary");
  summary.title = until > Date.now() ? "Звук отключен" : "Настройки чата";
  summary.innerHTML = `<i class="fa-solid ${until > Date.now() ? "fa-volume-xmark" : "fa-ellipsis"}" aria-hidden="true"></i>`;
}

function setMute(minutes) {
  const until = minutes === -1 ? 9999999999999 : minutes === 0 ? 0 : Date.now() + minutes * 60 * 1000;
  localStorage.setItem("chat_mute_until", String(until));
  updateMuteLabel();
  document.querySelector(".chat-preferences").open = false;
}

function playNotification() {
  const mutedUntil = Number(localStorage.getItem("chat_mute_until") || 0);
  if (Date.now() < mutedUntil) return;
  const audio = new Audio("https://assets.mixkit.co/active_storage/sfx/2358/2358-preview.mp3");
  audio.play().catch(() => {});
}

function openAvatarDialog() {
  avatarDraft = currentUser.avatar_url || makeAvatarUrl(currentUser.username);
  byId("chat-avatar-style").value = "identicon";
  byId("chat-profile-preview").src = avatarDraft;
  byId("chat-profile-modal").hidden = false;
}

function makeAvatarUrl(username, randomize = false) {
  const style = byId("chat-avatar-style")?.value || "identicon";
  const seed = randomize ? crypto.randomUUID() : username;
  return `https://api.dicebear.com/7.x/${style}/svg?seed=${encodeURIComponent(seed)}`;
}

async function saveAvatar() {
  try {
    const { data, error } = await client.from("profiles")
      .update({ avatar_url: avatarDraft })
      .eq("id", currentUser.id)
      .select()
      .single();
    if (error) throw error;
    currentUser = data;
    saveSession();
    byId("chat-user-avatar").src = currentUser.avatar_url;
    byId("chat-profile-modal").hidden = true;
    showChatNotice("Аватар обновлен.");
  } catch (error) {
    showChatNotice(error.message || "Не удалось обновить аватар.");
  }
}

async function deleteAccount() {
  const confirmed = await confirmChatAction("Удалить аккаунт чата? Это действие нельзя отменить.", {
    title: "Удаление аккаунта",
    confirmText: "Удалить аккаунт",
  });
  if (!confirmed) return;
  const { error } = await client.from("profiles").delete().eq("id", currentUser.id);
  if (error) return showChatNotice(error.message || "Не удалось удалить аккаунт.");
  logout();
}

function logout() {
  realtimeChannels.forEach((channel) => client.removeChannel(channel));
  realtimeChannels = [];
  typingChannel = null;
  localStorage.removeItem(CHAT_SESSION_KEY);
  window.location.reload();
}

function showChatNotice(message) {
  setAuthError(message);
  if (authGate.hidden) {
    const status = byId("chat-connection-status");
    status.textContent = message;
    status.classList.remove("is-connected");
    window.setTimeout(() => {
      if (currentUser) setConnectionStatus("Чат подключен", true);
    }, 5000);
  }
}

function setupPreferences() {
  document.querySelectorAll("[data-mute]").forEach((button) => {
    button.addEventListener("click", () => setMute(Number(button.dataset.mute)));
  });
  byId("chat-logout").addEventListener("click", logout);
  byId("chat-delete-account").addEventListener("click", deleteAccount);
  byId("chat-change-avatar").addEventListener("click", openAvatarDialog);
  byId("chat-avatar-style").addEventListener("change", () => {
    avatarDraft = makeAvatarUrl(currentUser.username);
    byId("chat-profile-preview").src = avatarDraft;
  });
  byId("chat-avatar-randomize").addEventListener("click", () => {
    avatarDraft = makeAvatarUrl(currentUser.username, true);
    byId("chat-profile-preview").src = avatarDraft;
  });
  byId("chat-avatar-save").addEventListener("click", saveAvatar);
  byId("chat-avatar-cancel").addEventListener("click", () => { byId("chat-profile-modal").hidden = true; });
}

setupComposer();
setupPreferences();
restoreSession();