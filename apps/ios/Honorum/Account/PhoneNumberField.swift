import SwiftUI
import UIKit

/// The mobile number entry, shaped as it is typed. UIKit's change hook is the
/// one place the proposed text and the cursor are known together, so the
/// separators land in the same keystroke as the digit and nothing typed
/// quickly is lost to a later rewrite. The cursor stays after the digit that
/// was just typed, wherever that is in the number.
struct PhoneNumberField: UIViewRepresentable {
    @Binding var national: String
    @Binding var country: PhoneCountries.Country
    @Binding var focused: Bool
    var placeholder = "Mobile number"

    func makeUIView(context: Context) -> FocusableTextField {
        let field = FocusableTextField()
        field.placeholder = placeholder
        field.accessibilityIdentifier = placeholder
        field.keyboardType = .phonePad
        field.textContentType = .telephoneNumber
        field.autocorrectionType = .no
        field.font = .monospacedDigitSystemFont(
            ofSize: UIFont.preferredFont(forTextStyle: .body).pointSize, weight: .regular)
        field.adjustsFontForContentSizeCategory = true
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        field.delegate = context.coordinator
        return field
    }

    func updateUIView(_ field: FocusableTextField, context: Context) {
        context.coordinator.parent = self
        if field.text != national { field.text = national }
        field.isEnabled = context.environment.isEnabled
        field.wantsFocus = focused
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: PhoneNumberField

        init(parent: PhoneNumberField) { self.parent = parent }

        func textField(_ field: UITextField, shouldChangeCharactersIn range: NSRange,
                       replacementString string: String) -> Bool {
            let current = field.text ?? ""
            guard let edited = Range(range, in: current) else { return true }
            let proposed = current.replacingCharacters(in: edited, with: string)
            let shaped = PhoneCountries.shaped(proposed, country: parent.country)
            // Keep the cursor after the digit it followed in the proposed text.
            let typedThrough = current.distance(from: current.startIndex, to: edited.lowerBound)
                + string.count
            let digitsBefore = proposed.prefix(typedThrough).filter(\.isNumber).count
            field.text = shaped.text
            place(cursorAfterDigit: digitsBefore, in: field)
            if shaped.country.id != parent.country.id { parent.country = shaped.country }
            parent.national = shaped.text
            return false
        }

        func textFieldDidBeginEditing(_ field: UITextField) {
            if !parent.focused { parent.focused = true }
        }

        func textFieldDidEndEditing(_ field: UITextField) {
            if parent.focused { parent.focused = false }
        }

        private func place(cursorAfterDigit count: Int, in field: UITextField) {
            let text = field.text ?? ""
            var seen = 0
            var offset = text.count
            for (index, character) in text.enumerated() where character.isNumber {
                seen += 1
                if seen == count { offset = index + 1; break }
            }
            if count == 0 { offset = 0 }
            guard let position = field.position(from: field.beginningOfDocument, offset: offset) else { return }
            field.selectedTextRange = field.textRange(from: position, to: position)
        }
    }
}

/// A text field that takes or gives up the keyboard to match a flag, waiting
/// for a window when asked before it has one.
final class FocusableTextField: UITextField {
    var wantsFocus = false {
        didSet { if wantsFocus != oldValue { syncFocus() } }
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        syncFocus()
    }

    private func syncFocus() {
        guard window != nil else { return }
        if wantsFocus, !isFirstResponder {
            becomeFirstResponder()
        } else if !wantsFocus, isFirstResponder {
            resignFirstResponder()
        }
    }
}
