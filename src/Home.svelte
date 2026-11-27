<script lang="ts">
  type BookmarkLink = { id: number; title: string; url: string };
  type Props = {
    bookmarks: BookmarkLink[];
    onSearch: (query: string) => void;
    onNavigate: (url: string) => void;
  };

  let { bookmarks, onSearch, onNavigate }: Props = $props();
  let query = $state("");
</script>

<section class="home" aria-label="Eefi home">
  <div class="home-content">
    <div class="home-brand" aria-label="Eefi Browser">eefi</div>
    <h1>A calmer corner of the web.</h1>
    <p class="home-intro">Where would you like to go?</p>

    <form
      class="home-search"
      role="search"
      onsubmit={(event) => {
        event.preventDefault();
        const search = query.trim();
        if (search) onSearch(search);
      }}
    >
      <span aria-hidden="true">⌕</span>
      <input
        bind:value={query}
        aria-label="Search or enter an address"
        autocomplete="off"
        autocapitalize="off"
        placeholder="Search or enter an address"
        spellcheck="false"
      />
      <button type="submit" aria-label="Search">↵</button>
    </form>

    {#if bookmarks.length}
      <section class="home-shortcuts" aria-labelledby="shortcuts-heading">
        <h2 id="shortcuts-heading">Your shortcuts</h2>
        <div class="shortcut-grid">
          {#each bookmarks.slice(0, 8) as bookmark (bookmark.id)}
            <button
              class="shortcut"
              title={bookmark.url}
              onclick={() => onNavigate(bookmark.url)}
            >
              <span class="shortcut-icon" aria-hidden="true">{bookmark.title.slice(0, 1).toUpperCase()}</span>
              <span class="shortcut-title">{bookmark.title}</span>
            </button>
          {/each}
        </div>
      </section>
    {:else}
      <p class="home-empty">Bookmarks on your bar will appear here as shortcuts.</p>
    {/if}
  </div>
</section>

<style>
  .home {
    display: grid;
    width: 100%;
    min-height: 100%;
    padding: 48px 24px;
    place-items: center;
    overflow: auto;
    color: #e8eaf0;
    background:
      radial-gradient(ellipse at 50% 35%, rgb(91 111 171 / 15%), transparent 48%),
      #11131a;
  }

  .home-content {
    width: min(660px, 100%);
    margin: auto;
    text-align: center;
  }

  .home-brand {
    color: #bac8ff;
    font-size: 30px;
    font-weight: 750;
    letter-spacing: 0.1em;
  }

  h1 {
    margin: 18px 0 8px;
    font-size: clamp(26px, 4vw, 36px);
    font-weight: 600;
    letter-spacing: -0.035em;
  }

  .home-intro,
  .home-empty {
    color: #9ba2b2;
  }

  .home-intro {
    margin: 0 0 28px;
    font-size: 14px;
  }

  .home-search {
    display: flex;
    align-items: center;
    height: 54px;
    padding: 0 9px 0 17px;
    gap: 12px;
    border: 1px solid #363c4b;
    border-radius: 15px;
    background: #191c25;
    box-shadow: 0 12px 35px rgb(0 0 0 / 22%);
  }

  .home-search > span {
    color: #9ba2b2;
    font-size: 24px;
  }

  .home-search input {
    width: 100%;
    min-width: 0;
    height: 100%;
    color: #eef0f5;
    border: 0;
    outline: 0;
    background: transparent;
    font-size: 15px;
  }

  .home-search input::placeholder {
    color: #81899b;
  }

  .home-search button {
    width: 36px;
    height: 36px;
    flex: 0 0 auto;
    color: #dce3ff;
    border: 0;
    border-radius: 10px;
    background: #30394f;
    cursor: pointer;
  }

  .home-search button:hover {
    background: #3c4968;
  }

  .home-shortcuts {
    margin-top: 42px;
    text-align: left;
  }

  .home-shortcuts h2 {
    margin: 0 0 15px 3px;
    color: #aeb5c4;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .shortcut-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(112px, 1fr));
    gap: 10px;
  }

  .shortcut {
    display: flex;
    min-width: 0;
    min-height: 90px;
    align-items: center;
    flex-direction: column;
    justify-content: center;
    padding: 12px 8px;
    gap: 9px;
    color: #dfe2e9;
    border: 1px solid transparent;
    border-radius: 13px;
    background: rgb(25 28 37 / 80%);
    cursor: pointer;
  }

  .shortcut:hover {
    border-color: #363c4b;
    background: #222631;
  }

  .shortcut-icon {
    display: grid;
    width: 34px;
    height: 34px;
    place-items: center;
    color: #dce3ff;
    border-radius: 11px;
    background: #30394f;
    font-size: 15px;
    font-weight: 650;
  }

  .shortcut-title {
    width: 100%;
    overflow: hidden;
    color: #c3c8d3;
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .home-empty {
    margin-top: 34px;
    font-size: 12px;
  }

  @media (max-width: 600px) {
    .home {
      padding: 28px 16px;
    }

    .shortcut-grid {
      grid-template-columns: repeat(auto-fill, minmax(90px, 1fr));
    }
  }
</style>
