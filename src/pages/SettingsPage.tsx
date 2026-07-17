import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { Check, CheckCircle2, FileText, Folder, Globe2, Info, LockKeyhole, Moon, RotateCcw, Save, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";
import { useSettings, type Appearance } from "../settings";
import type { Language } from "../i18n";

const copy = {
  en: {
    eyebrow: "FILENOVA PREFERENCES", title: "Settings", subtitle: "Personalize FileNova while keeping your files private and protected.",
    language: "Language", languageHint: "The interface updates immediately.", appearance: "Appearance", appearanceHint: "Choose how FileNova looks.", system: "System", light: "Light", dark: "Dark",
    output: "Default output folder", outputHint: "Use one folder for new results, or choose the destination each time.", askEveryTime: "Ask every time", chooseFolder: "Choose Folder", clearFolder: "Use Ask every time",
    fileBehavior: "File behavior", keepOriginal: "Keep original files", neverOverwrite: "Never overwrite existing files", lockedSafety: "Always enabled for your safety", openAfter: "Open output folder after processing",
    privacy: "Privacy", privacyTitle: "Your files stay on this device", privacyText: "Files are processed locally. No file or personal data is sent to a server.", history: "Enable processing history", historyHint: "Store a local list of completed operations on this device.",
    about: "About", platform: "Windows x64", licenses: "Open Licenses", documentation: "Open Documentation", components: "Local components", available: "Available", unavailable: "Unavailable", checking: "Checking…",
    save: "Save Settings", reset: "Reset to Defaults", saved: "Settings saved successfully.", resetDone: "Default settings restored.", error: "The settings could not be saved. Please try again.",
  },
  fr: {
    eyebrow: "PRÉFÉRENCES FILENOVA", title: "Paramètres", subtitle: "Personnalisez FileNova tout en gardant vos fichiers privés et protégés.",
    language: "Langue", languageHint: "L’interface est mise à jour immédiatement.", appearance: "Apparence", appearanceHint: "Choisissez l’apparence de FileNova.", system: "Système", light: "Clair", dark: "Sombre",
    output: "Dossier de sortie par défaut", outputHint: "Utilisez un dossier pour les nouveaux résultats ou choisissez la destination à chaque fois.", askEveryTime: "Demander à chaque fois", chooseFolder: "Choisir un dossier", clearFolder: "Demander à chaque fois",
    fileBehavior: "Comportement des fichiers", keepOriginal: "Conserver les fichiers originaux", neverOverwrite: "Ne jamais écraser les fichiers existants", lockedSafety: "Toujours activé pour votre sécurité", openAfter: "Ouvrir le dossier de sortie après le traitement",
    privacy: "Confidentialité", privacyTitle: "Vos fichiers restent sur cet appareil", privacyText: "Les fichiers sont traités localement. Aucun fichier ni aucune donnée personnelle n’est envoyé à un serveur.", history: "Activer l’historique des traitements", historyHint: "Conserver sur cet appareil une liste locale des opérations terminées.",
    about: "À propos", platform: "Windows x64", licenses: "Ouvrir les licences", documentation: "Ouvrir la documentation", components: "Composants locaux", available: "Disponible", unavailable: "Indisponible", checking: "Vérification…",
    save: "Enregistrer les paramètres", reset: "Réinitialiser", saved: "Paramètres enregistrés.", resetDone: "Paramètres par défaut restaurés.", error: "Impossible d’enregistrer les paramètres. Réessayez.",
  },
  ar: {
    eyebrow: "تفضيلات FILENOVA", title: "الإعدادات", subtitle: "خصّص FileNova مع الحفاظ على خصوصية ملفاتك وحمايتها.",
    language: "اللغة", languageHint: "يتم تحديث الواجهة فورًا.", appearance: "المظهر", appearanceHint: "اختر مظهر FileNova.", system: "النظام", light: "فاتح", dark: "داكن",
    output: "مجلد الإخراج الافتراضي", outputHint: "استخدم مجلدًا واحدًا للنتائج الجديدة أو اختر الوجهة في كل مرة.", askEveryTime: "السؤال في كل مرة", chooseFolder: "اختيار مجلد", clearFolder: "السؤال في كل مرة",
    fileBehavior: "سلوك الملفات", keepOriginal: "الاحتفاظ بالملفات الأصلية", neverOverwrite: "عدم استبدال الملفات الموجودة", lockedSafety: "مفعّل دائمًا لحمايتك", openAfter: "فتح مجلد الإخراج بعد المعالجة",
    privacy: "الخصوصية", privacyTitle: "تبقى ملفاتك على هذا الجهاز", privacyText: "تتم معالجة الملفات محليًا. لا يُرسل أي ملف أو بيانات شخصية إلى خادم.", history: "تفعيل سجل المعالجة", historyHint: "حفظ قائمة محلية بالعمليات المكتملة على هذا الجهاز.",
    about: "حول التطبيق", platform: "Windows x64", licenses: "فتح التراخيص", documentation: "فتح الوثائق", components: "المكونات المحلية", available: "متاح", unavailable: "غير متاح", checking: "جارٍ التحقق…",
    save: "حفظ الإعدادات", reset: "استعادة الإعدادات الافتراضية", saved: "تم حفظ الإعدادات بنجاح.", resetDone: "تمت استعادة الإعدادات الافتراضية.", error: "تعذر حفظ الإعدادات. حاول مرة أخرى.",
  },
} as const;
type Copy = (typeof copy)[keyof typeof copy];

interface RuntimeStatus { pdfium: boolean; ffmpeg: boolean; ffprobe: boolean }

export function SettingsPage() {
  const { settings, updateSettings, saveSettings, resetSettings } = useSettings();
  const c: Copy = copy[settings.language];
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [notice, setNotice] = useState<{ text: string; error?: boolean } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => { void invoke<RuntimeStatus>("get_runtime_status").then(setStatus).catch(() => setStatus({ pdfium: false, ffmpeg: false, ffprobe: false })); }, []);

  const chooseFolder = async () => {
    const selected = await open({ directory: true, multiple: false, title: c.chooseFolder });
    if (typeof selected === "string") updateSettings({ defaultOutputFolder: selected });
  };

  const save = async () => {
    setSaving(true); setNotice(null);
    try { await saveSettings(); setNotice({ text: c.saved }); }
    catch { setNotice({ text: c.error, error: true }); }
    finally { setSaving(false); }
  };

  const reset = async () => {
    setSaving(true); setNotice(null);
    try { await resetSettings(); setNotice({ text: c.resetDone }); }
    catch { setNotice({ text: c.error, error: true }); }
    finally { setSaving(false); }
  };

  const openDocument = async (document: "licenses" | "documentation") => {
    try { await openPath(await invoke<string>("get_support_document_path", { document })); }
    catch { setNotice({ text: c.error, error: true }); }
  };

  const toggle = (checked: boolean, label: string, locked = false, onChange?: () => void) => <label className={`settings-toggle${locked ? " locked" : ""}`}>
    <input checked={checked} disabled={locked} onChange={onChange} type="checkbox"/>
    <span className="settings-check"><Check size={13}/></span><span><strong>{label}</strong>{locked && <small><LockKeyhole size={11}/>{c.lockedSafety}</small>}</span>
  </label>;

  return <div className="page settings-page">
    <header className="page-header settings-header"><div><span className="eyebrow">{c.eyebrow}</span><h1>{c.title}</h1><p>{c.subtitle}</p></div><ShieldCheck size={36}/></header>
    <div className="settings-grid">
      <section className="settings-card"><div className="settings-card-title"><Globe2/><div><h2>{c.language}</h2><p>{c.languageHint}</p></div></div><div className="settings-segment three">
        {([['en','English'],['fr','Français'],['ar','العربية']] as [Language,string][]).map(([value,label]) => <button className={settings.language === value ? "active" : ""} key={value} onClick={() => updateSettings({ language: value })}>{label}</button>)}
      </div></section>
      <section className="settings-card"><div className="settings-card-title"><Moon/><div><h2>{c.appearance}</h2><p>{c.appearanceHint}</p></div></div><div className="settings-segment three">
        {([['system',c.system],['light',c.light],['dark',c.dark]] as [Appearance,string][]).map(([value,label]) => <button className={settings.appearance === value ? "active" : ""} key={value} onClick={() => updateSettings({ appearance: value })}>{label}</button>)}
      </div></section>
      <section className="settings-card settings-wide"><div className="settings-card-title"><Folder/><div><h2>{c.output}</h2><p>{c.outputHint}</p></div></div><div className="settings-folder"><span title={settings.defaultOutputFolder ?? c.askEveryTime}>{settings.defaultOutputFolder ?? c.askEveryTime}</span><button className="secondary-button" onClick={() => void chooseFolder()}>{c.chooseFolder}</button>{settings.defaultOutputFolder && <button className="text-button" onClick={() => updateSettings({ defaultOutputFolder: null })}>{c.clearFolder}</button>}</div></section>
      <section className="settings-card"><div className="settings-card-title"><FileText/><div><h2>{c.fileBehavior}</h2></div></div><div className="settings-options">{toggle(true,c.keepOriginal,true)}{toggle(true,c.neverOverwrite,true)}{toggle(settings.openOutputFolderAfterProcessing,c.openAfter,false,() => updateSettings({ openOutputFolderAfterProcessing: !settings.openOutputFolderAfterProcessing }))}</div></section>
      <section className="settings-card"><div className="settings-card-title"><ShieldCheck/><div><h2>{c.privacy}</h2></div></div><div className="privacy-callout"><LockKeyhole/><span><strong>{c.privacyTitle}</strong><small>{c.privacyText}</small></span></div><div className="settings-options">{toggle(settings.enableProcessingHistory,c.history,false,() => updateSettings({ enableProcessingHistory: !settings.enableProcessingHistory }))}<p className="settings-hint">{c.historyHint}</p></div></section>
      <section className="settings-card settings-wide"><div className="settings-card-title"><Info/><div><h2>{c.about}</h2><p>FileNova 0.2.0 · {c.platform}</p></div></div><div className="about-layout"><div className="about-actions"><button className="secondary-button" onClick={() => void openDocument('licenses')}>{c.licenses}</button><button className="secondary-button" onClick={() => void openDocument('documentation')}>{c.documentation}</button></div><div className="runtime-status"><strong>{c.components}</strong>{(['pdfium','ffmpeg','ffprobe'] as const).map((name) => <span key={name}><b>{name === 'pdfium' ? 'PDFium' : name === 'ffmpeg' ? 'FFmpeg' : 'FFprobe'}</b><em className={status?.[name] ? 'ready' : ''}>{status ? (status[name] ? c.available : c.unavailable) : c.checking}</em></span>)}</div></div></section>
    </div>
    <footer className="settings-actions"><div>{notice && <span className={notice.error ? "settings-notice error" : "settings-notice"}><CheckCircle2/>{notice.text}</span>}</div><button className="secondary-button" disabled={saving} onClick={() => void reset()}><RotateCcw/>{c.reset}</button><button className="primary-button" disabled={saving} onClick={() => void save()}><Save/>{c.save}</button></footer>
  </div>;
}
