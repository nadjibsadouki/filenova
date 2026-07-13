import { useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { navigationItems, type PageId } from "./navigation";
import { HomePage } from "./pages/HomePage";
import { PlaceholderPage } from "./pages/PlaceholderPage";

function App() {
  const [activePage, setActivePage] = useState<PageId>("home");
  const currentPage = navigationItems.find((item) => item.id === activePage)!;

  return (
    <div className="app-shell">
      <Sidebar activePage={activePage} onNavigate={setActivePage} />
      <main className="main-content">
        {activePage === "home" ? (
          <HomePage onNavigate={setActivePage} />
        ) : (
          <PlaceholderPage page={currentPage} />
        )}
      </main>
    </div>
  );
}

export default App;
